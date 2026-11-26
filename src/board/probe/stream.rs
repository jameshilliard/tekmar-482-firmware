use super::super::baud::Acquisition;
use super::super::output::{Kind, Output};
use super::super::panel::Panel;
use super::super::transmit::{Attempt, LineDriver};
use super::{CCR, CR0, Capture, INPUTS, IR, TC, TIMERS, read, write};
use core::cell::RefCell;
use embassy_futures::yield_now;
use embassy_time::Instant;
use tekmar_482_firmware::{
    inventory::{Answer, GroupSend, Inventory},
    tn4::{
        Frame, LinkAck, Receiver, Trailer,
        tx::{Outcome, Request},
    },
    tpck,
    trpc::{AfterDrain, Call},
};

pub struct Receivers {
    _capture: Capture,
    rx: [Receiver; 5],
    rising: [bool; 5],
}

impl Receivers {
    pub fn start() -> Self {
        let capture = Capture::arm(true);
        let high = INPUTS.map(|(pin, _, _)| capture.initial & (1 << pin) != 0);
        Self {
            rx: high.map(Receiver::starting_at),
            rising: high.map(|level| !level),
            _capture: capture,
        }
    }

    pub fn poll(&mut self, now: [u32; 2], mut report: impl FnMut(usize, Frame<'_>)) {
        let pending = TIMERS.map(|t| read(t + IR));
        for (port, &(_, timer, ch)) in INPUTS.iter().enumerate() {
            let t = TIMERS[timer];
            let flag = 1 << (4 + ch);
            let mut emit = |f: Frame<'_>| report(port, f);
            if pending[timer] & flag != 0 {
                let at = read(t + CR0 + 4 * ch);
                write(t + IR, flag);
                let next = if self.rising[port] { 6 } else { 5 };
                write(
                    t + CCR,
                    (read(t + CCR) & !(7 << (3 * ch))) | (next << (3 * ch)),
                );
                self.rx[port].edge(at, self.rising[port], &mut emit);
                self.rising[port] = !self.rising[port];
            }
            self.rx[port].advance(now[timer], emit);
        }
    }
}
fn stamp() -> u32 {
    Instant::now().as_millis() as u32
}

#[derive(Clone, Copy)]
enum NormalTx {
    Probe,
    Setting,
    Group(GroupSend),
}

pub async fn run(panel: &RefCell<Panel>) -> ! {
    static INVENTORY: static_cell::ConstStaticCell<Inventory> =
        static_cell::ConstStaticCell::new(Inventory::new());
    let inventory = INVENTORY.take();
    inventory.set_reporting(panel.borrow().report()[0] & 1 != 0);
    let mut bus = Receivers::start();
    let mut decoder = tpck::Decoder::default();
    let out = Output::take();
    let mut baud = Acquisition::default();
    let mut raw = false;
    let mut reboot = false;
    let mut occupied = [false; 5];
    let mut owner = [NormalTx::Probe; 5];
    #[unsafe(link_section = ".low_bss")]
    static SENDERS: static_cell::StaticCell<[Option<Attempt>; 5]> = static_cell::StaticCell::new();
    let sending = SENDERS.init_with(|| core::array::from_fn(|_| None));
    let mut ack: [LinkAck; 5] = core::array::from_fn(|_| LinkAck::default());
    let mut ack_pin: [Option<LineDriver>; 5] = core::array::from_fn(|_| None);
    let mut ack_mask = 0u8;
    loop {
        let now = TIMERS.map(|t| read(t + TC));
        bus.poll(now, |p, frame| {
            panel.borrow_mut().observe(p);
            if !matches!(frame.trailer, Trailer::Ack | Trailer::NoAck) {
                return;
            }
            if raw {
                let mut data = [0; 73];
                data[1] = p as u8;
                data[2..6].copy_from_slice(&stamp().to_le_bytes());
                data[6..6 + frame.bytes.len()].copy_from_slice(frame.bytes);
                out.enqueue(Kind::Rx(p as u8), tpck::TN4, &data[..6 + frame.bytes.len()]);
            } else if !reboot
                && let Some(body) = inventory.observe(
                    p,
                    frame.bytes,
                    Instant::now().as_secs() as u32,
                    sending[p].is_none()
                        && !inventory.has_command(p)
                        && !inventory.has_group_send(p),
                )
                && let Some(req) = Request::new(p as u8, &body)
            {
                sending[p] = Attempt::new(&req, INPUTS[p].0, now[INPUTS[p].1]);
            }
        });
        for p in 0..5 {
            if let Some((why, bits, bytes)) = bus.rx[p].take_failure()
                && raw
            {
                let mut data = [0; 78];
                data[0] = 4;
                data[1] = p as u8;
                data[2..6].copy_from_slice(&stamp().to_le_bytes());
                data[6] = why;
                data[7..9].copy_from_slice(&bits.to_le_bytes());
                data[9..9 + bytes.len()].copy_from_slice(bytes);
                out.enqueue(Kind::Invalid(p as u8), tpck::TN4, &data[..9 + bytes.len()]);
            }
            let t = read(TIMERS[INPUTS[p].1] + TC);
            if !raw
                && (inventory.has_command(p)
                    || inventory.has_group_send(p)
                    || matches!(owner[p], NormalTx::Group(_)))
            {
                let seconds = Instant::now().as_secs() as u32;
                inventory.command_expire(p, seconds);
                let cancel = match owner[p] {
                    NormalTx::Probe => true,
                    NormalTx::Setting => inventory.command_finished(p),
                    NormalTx::Group(tx) => inventory.stale_group_send(p, tx, seconds),
                };
                if cancel && sending[p].as_ref().is_some_and(|a| !a.started()) {
                    sending[p] = None;
                    owner[p] = NormalTx::Probe;
                }
                if sending[p].is_none() {
                    if let Some(req) = inventory.command_frame(p) {
                        sending[p] = Attempt::new(&req, INPUTS[p].0, t);
                        if sending[p].is_some() {
                            owner[p] = NormalTx::Setting;
                        } else {
                            inventory.command_sent(p, false);
                        }
                    } else if out.can_receive()
                        && let Some(reply) = inventory.command_reply(p)
                    {
                        out.enqueue(Kind::Control, tpck::TRPC, reply.payload());
                    } else if !inventory.has_command(p)
                        && let Some(tx) = inventory.next_group_send(p, seconds)
                        && let Some(req) = tx.frame(p as u8)
                    {
                        sending[p] = Attempt::new(&req, INPUTS[p].0, t);
                        if sending[p].is_some() {
                            owner[p] = NormalTx::Group(tx);
                        }
                    }
                }
            }
            if let Some(attempt) = sending[p].as_mut() {
                if let Some(result) = attempt.poll(t) {
                    sending[p] = None;
                    if !raw {
                        if matches!(
                            core::mem::replace(&mut owner[p], NormalTx::Probe),
                            NormalTx::Setting
                        ) {
                            inventory
                                .command_sent(p, matches!(result, Outcome::Ack | Outcome::NoAck));
                        }
                        continue;
                    }
                    let why = match result {
                        Outcome::Ack => 0,
                        Outcome::NoAck => 1,
                        Outcome::ArbitrationLost => 2,
                        Outcome::EchoMismatch => 3,
                        Outcome::BadEnd => 4,
                        Outcome::Late => 5,
                    };
                    let body = [2, p as u8, if why == 0 { 0 } else { 2 }, why];
                    out.enqueue(
                        Kind::Result(p as u8),
                        tpck::TN4,
                        if why == 0 { &body[..3] } else { &body },
                    );
                }
            } else if !reboot && raw && ack_mask & (1 << p) != 0 {
                if ack_pin[p].is_none() {
                    ack_pin[p] = LineDriver::claim(p as u8, INPUTS[p].0);
                }
                if let Some(driver) = ack_pin[p].as_mut()
                    && let Some(level) = ack[p].poll(t, bus.rx[p].ack_slot())
                {
                    driver.drive(level);
                }
            }
        }
        if out.can_receive()
            && let Some(reply) = inventory.walk_next()
        {
            out.enqueue(Kind::Control, tpck::TRPC, reply.payload());
        }
        'receive: {
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
                            if let [1, port, body @ ..] = packet.data {
                                let p = usize::from(*port);
                                if p >= 5 {
                                    out.enqueue(Kind::Control, tpck::TN4, &[3, *port, 2]);
                                } else if occupied[p] {
                                    out.enqueue(Kind::Overlap(*port), tpck::TN4, &[3, *port, 1]);
                                } else {
                                    occupied[p] = true;
                                    let mut reason = 0;
                                    if !raw {
                                        reason = 3;
                                    } else if let Some(req) = Request::new(*port, body) {
                                        if ack[p].driving() {
                                            reason = 4;
                                        } else {
                                            ack_pin[p] = None;
                                            sending[p] = Attempt::new(
                                                &req,
                                                INPUTS[p].0,
                                                read(TIMERS[INPUTS[p].1] + TC),
                                            );
                                            if sending[p].is_none() {
                                                reason = 2;
                                            }
                                        }
                                    } else {
                                        reason = 1;
                                    }
                                    if reason != 0 {
                                        out.enqueue(
                                            Kind::Result(*port),
                                            tpck::TN4,
                                            &[2, *port, 1, reason],
                                        );
                                    }
                                }
                            }
                        } else if let Some(call) = Call::parse(packet) {
                            if !raw
                                && matches!(call.method, 0x544bf002 | 0x544bf005)
                                && call.service == 0
                            {
                                for (p, attempt) in sending.iter_mut().enumerate() {
                                    if matches!(owner[p], NormalTx::Probe)
                                        && attempt.as_ref().is_some_and(|a| !a.started())
                                    {
                                        *attempt = None;
                                    }
                                }
                            }
                            let busy = inventory.walking()
                                || (0..5).any(|p| {
                                    inventory.has_command(p) || inventory.has_group_send(p)
                                })
                                || occupied.contains(&true)
                                || sending.iter().any(Option::is_some)
                                || ack.iter().any(LinkAck::driving);
                            let reply = match (call.method, call.service, call.body) {
                                (0x544bf002, 0, [m @ 0..=1]) if !busy => {
                                    if raw != (*m == 1) {
                                        inventory.reset();
                                    }
                                    raw = *m == 1;
                                    if !raw {
                                        ack_mask = 0;
                                        ack_pin = core::array::from_fn(|_| None);
                                    }
                                    call.ok(&[*m])
                                }
                                (0x544bf002, 0, [_]) if busy => call.error(1),
                                (0x544bf003, 1, [p @ 0..=4]) => {
                                    let mut b = [0; 9];
                                    b[0] = *p;
                                    for (chunk, n) in b[1..]
                                        .as_chunks_mut::<4>()
                                        .0
                                        .iter_mut()
                                        .zip(out.drops[*p as usize])
                                    {
                                        chunk.copy_from_slice(&n.to_le_bytes());
                                    }
                                    call.ok(&b)
                                }
                                (0x544bf007, 1, []) => {
                                    let mut b = [0; 28];
                                    for (chunk, v) in b
                                        .as_chunks_mut::<4>()
                                        .0
                                        .iter_mut()
                                        .zip(panel.borrow().report())
                                    {
                                        chunk.copy_from_slice(&v.to_le_bytes());
                                    }
                                    call.ok(&b)
                                }
                                (0x544bf008, 0, []) => {
                                    panel.borrow_mut().start_test();
                                    call.ok(&[])
                                }
                                (0x544bf009, 0, [mask @ 0..=31]) if raw && !busy => {
                                    for (p, pin) in ack_pin.iter_mut().enumerate() {
                                        if mask & (1 << p) == 0 {
                                            *pin = None;
                                        }
                                    }
                                    ack_mask = *mask;
                                    call.ok(&[*mask])
                                }
                                (0x544bf009, 0, [_]) if busy => call.error(1),
                                (0x544bf00a, 1, []) => {
                                    let mut b = [0; 48];
                                    let counts = bus.rx.each_ref().map(|r| r.stats.edges);
                                    let stamps =
                                        INPUTS.map(|(_, t, ch)| read(TIMERS[t] + CR0 + 4 * ch));
                                    for (chunk, n) in b
                                        .as_chunks_mut::<4>()
                                        .0
                                        .iter_mut()
                                        .zip(now.into_iter().chain(counts).chain(stamps))
                                    {
                                        chunk.copy_from_slice(&n.to_le_bytes());
                                    }
                                    call.ok(&b)
                                }
                                (0x544bf002 | 0x544bf003 | 0x544bf007..=0x544bf00a, _, _) => {
                                    call.error(2)
                                }
                                _ => {
                                    if let Some(reply) = call.management(raw, busy) {
                                        reply
                                    } else if raw {
                                        call.error(3)
                                    } else {
                                        match inventory
                                            .handle(&call, Instant::now().as_secs() as u32)
                                        {
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
                    }
                }
                Ok(None) => {}
            }
        }
        if !raw
            && !reboot
            && out.can_report()
            && inventory.has_reports()
            && let Some(report) = inventory.next_report(Instant::now().as_secs() as u32)
        {
            out.enqueue(Kind::Control, tpck::TRPC, report.payload());
        }
        match out.pump() {
            Some(Kind::Result(p)) => occupied[p as usize] = false,
            Some(Kind::Reboot) => {
                drop(ack_pin);
                drop(bus);
                panel.borrow_mut().stop();
                super::super::enter_isp();
            }
            _ => {}
        }
        yield_now().await;
    }
}
