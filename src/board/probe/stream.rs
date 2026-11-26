use super::super::baud::Acquisition;
use super::super::lpc2146::{timer, uart0};
use super::super::output::{Kind, Output};
use super::super::panel::Panel;
use super::super::transmit::{Attempt, LineDriver};
use super::{CCR, Capture, CaptureInput, INPUTS, IR, TC, TIMERS, next_edge_flags, read, write};
use core::cell::RefCell;
use tekmar_482_firmware::tn4::{ALL_PORTS_MASK, LAST_PORT, MAX_BYTES, PORT_COUNT};
use tekmar_482_firmware::tpck::raw;
const MOVE_QUEUE_CAPACITY: usize = 128;
const NORMAL_MODE: u8 = 0;
const RAW_MODE: u8 = 1;
use embassy_futures::yield_now;
use tekmar_482_firmware::uptime::Uptime;
use tekmar_482_firmware::{
    inventory::{AddressMove, Answer, BlockProbe, Broadcast, Inventory, LineProbe},
    tn4::{
        Frame, LinkResponse, Receiver, Trailer,
        tx::{Mode, Outcome, Request},
    },
    tpck,
    trpc::{AfterDrain, BridgeError, Call, method, service},
};

pub struct Receivers {
    _capture: Capture,
    rx: [Receiver; PORT_COUNT],
    rising: [bool; PORT_COUNT],
}

impl Receivers {
    pub fn start() -> Self {
        let capture = Capture::arm();
        let high = INPUTS.map(|input| capture.initial & (1 << input.pin) != 0);
        Self {
            rx: high.map(Receiver::starting_at),
            rising: high.map(|level| !level),
            _capture: capture,
        }
    }

    pub fn poll(&mut self, now: [u32; 2], mut report: impl FnMut(usize, Frame<'_>)) {
        let pending = TIMERS.map(|t| read(t + IR));
        for (port, input) in INPUTS.iter().enumerate() {
            let (timer, ch) = (input.timer, input.channel);
            let t = TIMERS[timer];
            let flag = 1 << (timer::FIRST_CAPTURE_IRQ + ch);
            let mut emit = |f: Frame<'_>| report(port, f);
            if pending[timer] & flag != 0 {
                let at = input.timestamp();
                write(t + IR, flag);
                let next = next_edge_flags(self.rising[port]);
                write(
                    t + CCR,
                    (read(t + CCR)
                        & !(timer::CAPTURE_FIELD_MASK << (timer::CAPTURE_FIELD_BITS * ch)))
                        | (next << (timer::CAPTURE_FIELD_BITS * ch)),
                );
                self.rx[port].edge(at, self.rising[port], &mut emit);
                self.rising[port] = !self.rising[port];
            }
            self.rx[port].advance(now[timer], emit);
        }
    }
}
#[derive(Clone, Copy)]
enum NormalTx {
    Idle,
    Probe(BlockProbe),
    Discovery(LineProbe),
    Setting,
    Broadcast(Broadcast),
}

pub async fn run(panel: &RefCell<Panel>) -> ! {
    #[cfg_attr(not(feature = "ram-trial"), unsafe(link_section = ".bss.inventory"))]
    static INVENTORY: static_cell::ConstStaticCell<Inventory> =
        static_cell::ConstStaticCell::new(Inventory::new());
    let inventory = INVENTORY.take();
    #[unsafe(link_section = ".low_bss")]
    static MOVES: static_cell::ConstStaticCell<heapless::Deque<AddressMove, MOVE_QUEUE_CAPACITY>> =
        static_cell::ConstStaticCell::new(heapless::Deque::new());
    let moves = MOVES.take();
    inventory
        .set_reporting(panel.borrow().report()[0] & u32::from(super::super::panel::DIP_INPUT) != 0);
    let mut bus = Receivers::start();
    let mut decoder = tpck::Decoder::default();
    let out = Output::take();
    let mut baud = Acquisition::default();
    let mut raw = false;
    let mut reboot = false;
    let mut occupied = [false; PORT_COUNT];
    let mut owner = [NormalTx::Idle; PORT_COUNT];
    #[unsafe(link_section = ".low_bss")]
    static SENDERS: static_cell::StaticCell<[Option<Attempt>; PORT_COUNT]> =
        static_cell::StaticCell::new();
    let sending = SENDERS.init_with(|| core::array::from_fn(|_| None));
    let mut response: [LinkResponse; PORT_COUNT] =
        core::array::from_fn(|_| LinkResponse::default());
    let mut response_pin: [Option<LineDriver>; PORT_COUNT] = core::array::from_fn(|_| None);
    let mut ack_mask = 0u8;
    let mut uptime = Uptime::default();
    let mut last_service = None;
    loop {
        let us = crate::board::panel::now();
        let time = uptime.sample(us);
        let seconds = time.seconds;
        if !raw && !reboot && last_service != Some(seconds) {
            last_service = Some(seconds);
            inventory.tick(seconds);
        }
        let now = TIMERS.map(|t| read(t + TC));
        bus.poll(now, |p, frame| {
            panel.borrow_mut().observe(p);
            if !matches!(frame.trailer, Trailer::Ack | Trailer::NoAck) {
                return;
            }
            if raw {
                let mut data = [0; raw::RX_PREFIX + MAX_BYTES];
                data[0] = raw::RX;
                data[1] = p as u8;
                data[2..raw::RX_PREFIX].copy_from_slice(&time.millis.to_le_bytes());
                data[raw::RX_PREFIX..raw::RX_PREFIX + frame.bytes.len()]
                    .copy_from_slice(frame.bytes);
                out.enqueue(
                    Kind::Rx(p as u8),
                    tpck::TN4,
                    &data[..raw::RX_PREFIX + frame.bytes.len()],
                );
            } else if !reboot
                && !frame.early_start
                // RX sees our own frame before TX finishes. Only a peer counts as
                // link-up, suppresses a line probe or supplies an outdoor sample.
                && !sending[p].as_ref().is_some_and(Attempt::started)
                && let Some(moved) = inventory.observe(p, frame.bytes, seconds)
            {
                if moves.is_full() {
                    moves.pop_front();
                }
                let _ = moves.push_back(moved);
            }
        });
        for p in 0..PORT_COUNT {
            let listening = !reboot && !sending[p].as_ref().is_some_and(Attempt::started);
            let mut fault_at = bus.rx[p].take_drive_fault();
            if fault_at.is_some() {
                inventory.line(p).sent(Outcome::EchoMismatch);
            }
            if let Some(at) = bus.rx[p].take_gap_fault()
                && !raw
                && listening
            {
                inventory.line(p).start_backoff();
                fault_at = Some(at);
            }
            if let Some(fault) = bus.rx[p].take_failure() {
                if raw {
                    let mut data = [0; raw::INVALID_PREFIX + MAX_BYTES + size_of::<u16>()];
                    data[0] = raw::RX_INVALID;
                    data[1] = p as u8;
                    data[2..raw::RX_PREFIX].copy_from_slice(&time.millis.to_le_bytes());
                    data[raw::RX_PREFIX] = fault.reason;
                    data[raw::RX_PREFIX + 1..raw::INVALID_PREFIX]
                        .copy_from_slice(&fault.bits.to_le_bytes());
                    data[raw::INVALID_PREFIX..raw::INVALID_PREFIX + fault.bytes.len()]
                        .copy_from_slice(fault.bytes);
                    out.enqueue(
                        Kind::Invalid(p as u8),
                        tpck::TN4,
                        &data[..raw::INVALID_PREFIX + fault.bytes.len()],
                    );
                } else if listening && fault_at.is_none() {
                    inventory.line(p).frame_error();
                    fault_at = Some(fault.next_bit);
                }
            }
            if let Some(at) = fault_at {
                response[p].recover(&mut bus.rx[p], at);
            }
            if !raw && bus.rx[p].stuck_dominant() {
                inventory.line(p).down();
            }
            let t = read(TIMERS[INPUTS[p].timer] + TC);
            let may_start =
                raw || inventory.line(p).may_start(t, bus.rx[p].idle()) && !bus.rx[p].recovering();
            if !raw && !reboot {
                let foreground = inventory.has_command(p) || inventory.has_broadcast(p);
                let discovery = inventory.line(p).pending();
                inventory.command_expire(p, seconds);
                let cancel = match owner[p] {
                    NormalTx::Probe(probe) => {
                        foreground || !inventory.probe_current(probe, seconds)
                    }
                    NormalTx::Idle => false,
                    NormalTx::Discovery(probe) => foreground || discovery != Some(probe),
                    NormalTx::Setting => inventory.command_finished(p),
                    NormalTx::Broadcast(tx) => inventory.stale_broadcast(p, tx, seconds),
                };
                if cancel
                    && !response[p].busy()
                    && sending[p].as_ref().is_some_and(|a| !a.started())
                {
                    sending[p] = None;
                    if let NormalTx::Probe(probe) = owner[p] {
                        inventory.defer_probe(probe, seconds);
                    }
                    owner[p] = NormalTx::Idle;
                }
                if (foreground || discovery.is_some() || inventory.has_reads(p))
                    && sending[p].is_none()
                    && !response[p].busy()
                {
                    if let Some(req) = inventory.command_frame(p) {
                        sending[p] = Attempt::new(&req, INPUTS[p].pin, t, Mode::Normal);
                        if sending[p].is_some() {
                            owner[p] = NormalTx::Setting;
                        } else {
                            inventory.command_unavailable(p);
                        }
                    } else if out.can_receive()
                        && let Some(reply) = inventory.command_reply(p)
                    {
                        out.enqueue(Kind::Control, tpck::TRPC, reply.payload());
                    } else if let Some(tx) = inventory.next_broadcast(p, seconds)
                        && let Some(req) = tx.frame(p as u8)
                    {
                        sending[p] = Attempt::new(&req, INPUTS[p].pin, t, Mode::Normal);
                        if sending[p].is_some() {
                            owner[p] = NormalTx::Broadcast(tx);
                        }
                    } else if !foreground
                        && let Some(probe) = discovery
                        && let Some(req) = probe.frame(p as u8)
                    {
                        sending[p] = Attempt::new(&req, INPUTS[p].pin, t, Mode::Normal);
                        if sending[p].is_some() {
                            owner[p] = NormalTx::Discovery(probe);
                        }
                    } else if !foreground
                        && let Some(probe) = inventory.next_probe(p, time)
                        && let Some(req) = Request::new(p as u8, &probe.body())
                    {
                        sending[p] = Attempt::new(&req, INPUTS[p].pin, t, Mode::Normal);
                        if sending[p].is_some() {
                            owner[p] = NormalTx::Probe(probe);
                        }
                    }
                }
            }
            // A queued send is still a receiver until it actually drives SOF.
            // Never acknowledge our own transmission. Raw RX ACK is opt-in.
            let ack_slot = if listening
                && (if raw {
                    ack_mask & (1 << p) != 0
                } else {
                    !bus.rx[p].early_start()
                }) {
                bus.rx[p].ack_slot()
            } else {
                None
            };
            if let Some(high) = response[p].poll(t, ack_slot) {
                if let Some(waiting) = sending[p].as_mut() {
                    waiting.respond(high);
                } else if high {
                    response_pin[p] = LineDriver::claim(p as u8, INPUTS[p].pin);
                    if let Some(pin) = response_pin[p].as_mut() {
                        pin.drive(true);
                    }
                } else {
                    response_pin[p] = None;
                }
                if high && !raw && (sending[p].is_some() || response_pin[p].is_some()) {
                    bus.rx[p].verify_response_drive();
                }
            }
            if !response[p].busy()
                && let Some(attempt) = sending[p].as_mut()
            {
                let result = attempt.poll(t, may_start);
                let Some((result, fault_at)) = result else {
                    continue;
                };
                if !raw
                    && result == Outcome::ArbitrationLost
                    && !matches!(owner[p], NormalTx::Setting)
                {
                    attempt.yield_arbitration(t);
                    continue;
                }
                sending[p] = None;
                if !raw {
                    if let Some(at) = fault_at {
                        response[p].recover(&mut bus.rx[p], at);
                    }
                    inventory.line(p).sent(result);
                    match core::mem::replace(&mut owner[p], NormalTx::Idle) {
                        NormalTx::Setting => inventory.command_sent(p, result),
                        NormalTx::Probe(probe) => inventory.probe_sent(probe, time.millis),
                        NormalTx::Discovery(probe) => inventory.line(p).finished(probe),
                        _ => {}
                    }
                    continue;
                }
                let why = match result {
                    Outcome::Ack => 0,
                    Outcome::NoAck => raw::attempted::NO_ACK,
                    Outcome::ArbitrationLost => raw::attempted::ARBITRATION_LOST,
                    Outcome::EchoMismatch => raw::attempted::ECHO_MISMATCH,
                    Outcome::BadEnd => raw::attempted::BAD_END,
                    Outcome::Late => raw::attempted::MISSED_TIMING,
                };
                let body = [
                    raw::TX_RESULT,
                    p as u8,
                    if result == Outcome::Ack {
                        raw::status::ACKNOWLEDGED
                    } else {
                        raw::status::ATTEMPTED
                    },
                    why,
                ];
                out.enqueue(
                    Kind::Result(p as u8),
                    tpck::TN4,
                    if result == Outcome::Ack {
                        &body[..3]
                    } else {
                        &body
                    },
                );
            }
        }
        if out.can_receive()
            && let Some(moved) = moves.pop_front()
        {
            out.enqueue(Kind::Control, tpck::TRPC, moved.report().payload());
        }
        if out.can_receive()
            && let Some(reply) = inventory.walk_next()
        {
            out.enqueue(Kind::Control, tpck::TRPC, reply.payload());
        }
        // Several TPCK frames can arrive back-to-back at the negotiated baud.
        // Empty the 16-byte UART FIFO, but return to the line timers after
        // dispatching one command even if more bytes are already available.
        'receive: for _ in 0..uart0::FIFO_BYTES {
            if reboot || !out.can_receive() {
                break 'receive;
            }
            baud.poll(now[0], &mut decoder);
            let byte = super::super::try_receive();
            if !baud.locked && byte != Ok(None) {
                baud.activity(now[0]);
            }
            match byte {
                Err(()) => decoder.reset(),
                Ok(Some(b)) => {
                    if let Some(packet) = decoder.push(b) {
                        baud.locked = true;
                        if packet.kind == tpck::TN4 {
                            if let [raw::SEND, port, body @ ..] = packet.data {
                                let p = usize::from(*port);
                                if p >= PORT_COUNT {
                                    out.enqueue(
                                        Kind::Control,
                                        tpck::TN4,
                                        &[raw::PORT_ERROR, *port, raw::port_error::INVALID_PORT],
                                    );
                                } else if occupied[p] {
                                    out.enqueue(
                                        Kind::Overlap(*port),
                                        tpck::TN4,
                                        &[
                                            raw::PORT_ERROR,
                                            *port,
                                            raw::port_error::SEND_IN_PROGRESS,
                                        ],
                                    );
                                } else {
                                    occupied[p] = true;
                                    let mut reason = None;
                                    if !raw {
                                        reason = Some(raw::rejected::WRONG_MODE);
                                    } else if let Some(req) = Request::new(*port, body) {
                                        if response[p].busy() {
                                            reason = Some(raw::rejected::LINK_RESPONSE_ACTIVE);
                                        } else {
                                            response_pin[p] = None;
                                            sending[p] = Attempt::new(
                                                &req,
                                                INPUTS[p].pin,
                                                read(TIMERS[INPUTS[p].timer] + TC),
                                                Mode::Raw,
                                            );
                                            if sending[p].is_none() {
                                                reason = Some(raw::rejected::PIN_UNAVAILABLE);
                                            }
                                        }
                                    } else {
                                        reason = Some(raw::rejected::INVALID_BODY);
                                    }
                                    if let Some(reason) = reason {
                                        out.enqueue(
                                            Kind::Result(*port),
                                            tpck::TN4,
                                            &[raw::TX_RESULT, *port, raw::status::REJECTED, reason],
                                        );
                                    }
                                }
                            }
                        } else if let Some(call) = Call::parse(packet) {
                            if !raw
                                && matches!(
                                    call.method,
                                    method::SET_BRIDGE_MODE | method::ENTER_FIRMWARE_UPDATE
                                )
                                && call.service == service::UPDATE
                            {
                                for (p, attempt) in sending.iter_mut().enumerate() {
                                    if !response[p].busy()
                                        && matches!(
                                            owner[p],
                                            NormalTx::Probe(_) | NormalTx::Discovery(_)
                                        )
                                        && attempt.as_ref().is_some_and(|a| !a.started())
                                    {
                                        *attempt = None;
                                        if let NormalTx::Probe(probe) = owner[p] {
                                            inventory.defer_probe(probe, seconds);
                                        }
                                        owner[p] = NormalTx::Idle;
                                    }
                                }
                            }
                            let busy = !moves.is_empty()
                                || inventory.walking()
                                || (0..PORT_COUNT).any(|p| {
                                    inventory.has_command(p) || inventory.has_broadcast(p)
                                })
                                || occupied.contains(&true)
                                || sending.iter().any(Option::is_some)
                                || response.iter().any(LinkResponse::busy);
                            let reply = match (call.method, call.service, call.body) {
                                (
                                    method::SET_BRIDGE_MODE,
                                    service::UPDATE,
                                    [m @ NORMAL_MODE..=RAW_MODE],
                                ) if !busy => {
                                    if raw != (*m == RAW_MODE) {
                                        inventory.reset();
                                        last_service = None;
                                        panel.borrow_mut().reset_sequence();
                                    }
                                    raw = *m == RAW_MODE;
                                    if !raw {
                                        ack_mask = 0;
                                        response_pin = core::array::from_fn(|_| None);
                                    }
                                    call.ok(&[*m])
                                }
                                (method::SET_BRIDGE_MODE, service::UPDATE, [_]) if busy => {
                                    call.error(BridgeError::Busy)
                                }
                                (method::PORT_DROPS, service::REQUEST, [p @ 0..=LAST_PORT]) => {
                                    let mut b = [0; 1 + 2 * size_of::<u32>()];
                                    b[0] = *p;
                                    for (chunk, n) in b[1..]
                                        .as_chunks_mut::<{ size_of::<u32>() }>()
                                        .0
                                        .iter_mut()
                                        .zip(out.drops[*p as usize])
                                    {
                                        chunk.copy_from_slice(&n.to_le_bytes());
                                    }
                                    call.ok(&b)
                                }
                                (method::PANEL, service::REQUEST, []) => {
                                    let mut b =
                                        [0; super::super::panel::REPORT_WORDS * size_of::<u32>()];
                                    for (chunk, v) in b
                                        .as_chunks_mut::<{ size_of::<u32>() }>()
                                        .0
                                        .iter_mut()
                                        .zip(panel.borrow().report())
                                    {
                                        chunk.copy_from_slice(&v.to_le_bytes());
                                    }
                                    call.ok(&b)
                                }
                                (method::LED_TEST, service::UPDATE, []) => {
                                    panel.borrow_mut().start_test();
                                    call.ok(&[])
                                }
                                (
                                    method::LINK_ACK_MASK,
                                    service::UPDATE,
                                    [mask @ 0..=ALL_PORTS_MASK],
                                ) if raw && !busy => {
                                    for (p, pin) in response_pin.iter_mut().enumerate() {
                                        if mask & (1 << p) == 0 {
                                            *pin = None;
                                        }
                                    }
                                    ack_mask = *mask;
                                    call.ok(&[*mask])
                                }
                                (method::LINK_ACK_MASK, service::UPDATE, [_]) if busy => {
                                    call.error(BridgeError::Busy)
                                }
                                (method::CAPTURE_COUNTERS, service::REQUEST, []) => {
                                    let mut b =
                                        [0; (TIMERS.len() + 2 * PORT_COUNT) * size_of::<u32>()];
                                    let counts = bus.rx.each_ref().map(|r| r.stats.edges);
                                    let stamps = INPUTS.map(CaptureInput::timestamp);
                                    for (chunk, n) in b
                                        .as_chunks_mut::<{ size_of::<u32>() }>()
                                        .0
                                        .iter_mut()
                                        .zip(now.into_iter().chain(counts).chain(stamps))
                                    {
                                        chunk.copy_from_slice(&n.to_le_bytes());
                                    }
                                    call.ok(&b)
                                }
                                (
                                    method::SET_BRIDGE_MODE
                                    | method::PORT_DROPS
                                    | method::PANEL..=method::CAPTURE_COUNTERS,
                                    _,
                                    _,
                                ) => call.error(BridgeError::InvalidRequest),
                                _ => {
                                    if let Some(reply) = call.management(raw, busy) {
                                        reply
                                    } else if raw {
                                        call.error(BridgeError::WrongMode)
                                    } else {
                                        match inventory.handle(&call, seconds) {
                                            Answer::Reply(reply) => reply,
                                            Answer::Unhandled => call.unsupported(),
                                            Answer::Silent => {
                                                break 'receive;
                                            }
                                        }
                                    }
                                }
                            };
                            reboot = reply.after_drain == AfterDrain::EnterRom;
                            out.enqueue(
                                if reboot { Kind::Reboot } else { Kind::Control },
                                tpck::TRPC,
                                reply.payload(),
                            );
                        }
                        break 'receive;
                    }
                }
                Ok(None) => break,
            }
        }
        if !raw
            && !reboot
            && out.can_receive()
            && let Some(notice) = panel.borrow_mut().take_notice()
        {
            out.enqueue(Kind::Control, tpck::TRPC, notice.report().payload());
        }
        if !raw
            && !reboot
            && out.can_report()
            && inventory.has_reports()
            && let Some(report) = inventory.next_report(seconds)
        {
            out.enqueue(Kind::Control, tpck::TRPC, report.payload());
        }
        match out.pump() {
            Some(Kind::Result(p)) => occupied[p as usize] = false,
            Some(Kind::Reboot) => {
                drop(response_pin);
                drop(bus);
                panel.borrow_mut().stop();
                super::super::enter_isp();
            }
            _ => {}
        }
        yield_now().await;
    }
}
