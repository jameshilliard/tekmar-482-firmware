use super::super::transmit::Attempt;
use super::super::{LSR, THRE, UART0};
use super::{CCR, CR0, Capture, INPUTS, IR, TC, TIMERS, read, send, write};
use tekmar_482_firmware::tn4::tx::Request;
use tekmar_482_firmware::tn4::{Frame, Receiver, Trailer};

fn hex(dst: &mut [u8], value: u32) {
    for (i, byte) in dst.iter_mut().rev().enumerate() {
        *byte = b"0123456789abcdef"[((value >> (4 * i)) & 15) as usize];
    }
}

struct Outbox {
    buf: [u8; 1024],
    tail: usize,
    len: usize,
    dropped: [u32; 5],
}

impl Outbox {
    fn frame(&mut self, port: usize, f: Frame<'_>) {
        let mut line = [0u8; 22 + 2 * 67];
        line[..20].copy_from_slice(b"F 0 00000000 0000 A ");
        line[2] += port as u8;
        hex(&mut line[4..12], f.start_us);
        hex(&mut line[13..17], u32::from(f.crc));
        line[18] = match f.trailer {
            Trailer::Ack => b'A',
            Trailer::NoAck => b'N',
            Trailer::BadDelimiter => b'D',
            Trailer::BadEnd { ack: true } => b'E',
            Trailer::BadEnd { ack: false } => b'M',
        };
        for (digits, &byte) in line[20..].as_chunks_mut::<2>().0.iter_mut().zip(f.bytes) {
            hex(digits, u32::from(byte));
        }
        let end = 20 + f.bytes.len() * 2;
        line[end..end + 2].copy_from_slice(b"\r\n");
        if end + 2 + 32 > self.buf.len() - self.len {
            self.dropped[port] = self.dropped[port].saturating_add(1);
            return;
        }
        self.append(&line[..end + 2]);
    }

    fn append(&mut self, line: &[u8]) {
        for &b in line {
            self.buf[(self.tail + self.len) % self.buf.len()] = b;
            self.len += 1;
        }
    }

    fn result(&mut self, port: u8, outcome: &[u8]) {
        self.append(&[b'T', b' ', b'0' + port, b' ']);
        self.append(outcome);
        self.append(b"\r\n");
    }

    fn pump(&mut self) {
        if self.len == 0 || read(LSR) & THRE == 0 {
            return;
        }
        // THRE indicates the 16-byte UART0 transmit FIFO is empty.
        for _ in 0..self.len.min(16) {
            write(UART0, u32::from(self.buf[self.tail]));
            self.tail = (self.tail + 1) % self.buf.len();
            self.len -= 1;
        }
    }
}

pub struct Receivers {
    capture: Capture,
    rx: [Receiver; 5],
    rising: [bool; 5],
    max_delay: [u32; 5],
}

impl Receivers {
    pub fn start() -> Self {
        Self {
            rx: core::array::from_fn(|_| Receiver::default()),
            rising: [true; 5],
            max_delay: [0; 5],
            capture: Capture::arm(5),
        }
    }

    pub fn poll(
        &mut self,
        now: [u32; 2],
        diagnostic: bool,
        mut report: impl FnMut(usize, Frame<'_>),
    ) {
        let pending = TIMERS.map(|t| read(t + IR));
        for (port, (_, timer, ch)) in INPUTS.into_iter().enumerate() {
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
                if diagnostic {
                    self.max_delay[port] = self.max_delay[port].max(read(t + TC).wrapping_sub(at));
                }
                self.rx[port].edge(at, self.rising[port], &mut emit);
                self.rising[port] = !self.rising[port];
            }
            self.rx[port].advance(now[timer], emit);
        }
    }

    pub fn idle(&mut self, panel: &mut super::super::panel::Panel) {
        self.poll(TIMERS.map(|t| read(t + TC)), false, |port, _| {
            panel.observe(port)
        });
    }
}

pub fn stream(diagnostic: bool, panel: &mut super::super::panel::Panel, request: Option<&Request>) {
    let mut out = Outbox {
        buf: [0; 1024],
        tail: 0,
        len: 0,
        dropped: [0; 5],
    };
    send(if request.is_some() {
        b"OK TX\r\n"
    } else if diagnostic {
        b"OK DIAG\r\n"
    } else {
        b"OK FRAMES\r\n"
    });
    let mut receivers = Receivers::start();
    let mut attempt = request.and_then(|r| Attempt::new(r, INPUTS[usize::from(r.port)].0, 0));
    if let Some(r) = request
        && attempt.is_none()
    {
        out.result(r.port, b"PIN-BUSY");
    }
    while read(LSR) & 1 == 0 {
        let now = TIMERS.map(|t| read(t + TC));
        if let Some(tx) = &mut attempt
            && let Some(outcome) = tx.poll(now[0])
        {
            drop(attempt.take()); // release the line before reporting the result
            if let Some(r) = request {
                out.result(r.port, outcome);
            }
        }
        receivers.poll(now, diagnostic, |port, f| {
            panel.observe(port);
            out.frame(port, f);
        });
        out.pump();
        panel.poll();
    }
    read(UART0); // any byte stops capture; subsequent commands use normal training
    if attempt.take().is_some()
        && let Some(r) = request
    {
        out.result(r.port, b"STOPPED");
    }
    drop(receivers.capture);
    while out.len != 0 {
        out.pump();
    }
    if diagnostic {
        for (port, r) in receivers.rx.iter().enumerate() {
            let s = &r.stats;
            send(&[b'D', b' ', b'0' + port as u8]);
            for n in [
                s.edges,
                s.starts,
                s.stuffing,
                s.crc_mismatch,
                s.ack,
                s.no_ack,
                s.bad_trailer,
                u32::from(r.partial()),
                out.dropped[port],
                receivers.max_delay[port],
                u32::from(s.last_received_crc),
                u32::from(s.last_expected_crc),
            ] {
                let mut word = *b" 00000000";
                hex(&mut word[1..], n);
                send(&word);
            }
            send(b"\r\n");
        }
    }
    let mut end = *b"END 00000000\r\n";
    hex(
        &mut end[4..12],
        out.dropped.into_iter().fold(0, u32::saturating_add),
    );
    send(&end);
}
