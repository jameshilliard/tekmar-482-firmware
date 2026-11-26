//! One bus attempt. No automatic retransmission or active error flags.
use super::BIT_US;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub port: u8,
    data: [u8; 67],
    len: usize,
}

impl Request {
    pub fn new(port: u8, body: &[u8]) -> Option<Self> {
        let payload_len = usize::from(*body.get(3)? & 63);
        if port >= 5 || body.len() != 4 + payload_len {
            return None;
        }
        let mut request = Self {
            port,
            data: [0; 67],
            len: body.len(),
        };
        request.data[..body.len()].copy_from_slice(body);
        Some(request)
    }
}

/// Cursor after SOF. A Send can be admitted without first computing all
/// the protected bits while back-to-back host frames fill the UART FIFO.
struct Protected {
    request: Request,
    index: usize,
    crc: u16,
    run: u8,
    last: bool,
}

impl Protected {
    fn new(request: &Request) -> Self {
        Self {
            request: request.clone(),
            index: 0,
            crc: 0x4599,
            run: 1,
            last: true,
        }
    }
    fn next(&mut self) -> Option<bool> {
        let i = self.index;
        let data_bits = self.request.len * 8;
        if i == data_bits + 15 {
            return None; // No stuffing between the final CRC bit and delimiter.
        }
        if self.run == 5 {
            self.last = !self.last;
            self.run = 1;
            return Some(self.last);
        }
        let bit = if i < data_bits {
            let bit = self.request.data[i / 8] & (0x80 >> (i % 8)) != 0;
            let feedback = (self.crc & 0x4000 != 0) ^ bit;
            self.crc = ((self.crc << 1) & 0x7fff) ^ if feedback { 0x4599 } else { 0 };
            bit
        } else {
            self.crc & (1 << (14 - (i - data_bits))) != 0
        };
        self.index += 1;
        self.run = if bit == self.last { self.run + 1 } else { 1 };
        self.last = bit;
        Some(bit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ack,
    NoAck,
    ArbitrationLost,
    EchoMismatch,
    BadEnd,
    Late,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Drive(bool),
    Finish(Outcome),
}

enum Phase {
    Idle,
    Sample,
    Boundary,
    Lost,
    Done,
}

#[derive(Clone, Copy)]
enum Symbol {
    Protected(bool),
    Trailer(u8),
}

pub struct Sender {
    bits: Protected,
    phase: Phase,
    quiet_since: u32,
    due: u32,
    symbol: Symbol,
    ack: bool,
}

impl Sender {
    pub fn new(request: &Request, now: u32) -> Self {
        Self {
            bits: Protected::new(request),
            phase: Phase::Idle,
            quiet_since: now,
            due: now,
            symbol: Symbol::Protected(true), // SOF
            ack: false,
        }
    }

    fn finish(&mut self, outcome: Outcome) -> Action {
        self.phase = if outcome == Outcome::ArbitrationLost {
            Phase::Lost
        } else {
            Phase::Done
        };
        Action::Finish(outcome)
    }

    pub fn started(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Only a lost protected-bit arbitration proves that our frame did not finish.
    pub fn retry_arbitration(&mut self, now: u32) -> bool {
        if !matches!(self.phase, Phase::Lost) {
            return false;
        }
        *self = Self::new(&self.bits.request, now);
        true
    }

    pub fn hold_if_waiting(&mut self, now: u32) -> bool {
        if self.started() {
            return false;
        }
        self.quiet_since = now;
        true
    }

    pub fn poll(&mut self, now: u32, high: bool) -> Action {
        match self.phase {
            Phase::Done | Phase::Lost => return Action::None,
            Phase::Idle => {
                if high {
                    self.quiet_since = now;
                }
                // Longer than stuffing-limited data runs, delimiter, EOF and intermission.
                if now.wrapping_sub(self.quiet_since) < 16 * BIT_US {
                    return Action::None;
                }
                self.due = now.wrapping_add(BIT_US / 2);
                self.phase = Phase::Sample;
                return Action::Drive(true);
            }
            _ => {}
        }
        let late = now.wrapping_sub(self.due) as i32;
        if late < 0 {
            return Action::None;
        }
        if late >= (BIT_US / 4) as i32 {
            return self.finish(Outcome::Late);
        }
        self.due = self.due.wrapping_add(BIT_US / 2);
        if matches!(self.phase, Phase::Sample) {
            match self.symbol {
                Symbol::Trailer(1) => self.ack = high,
                Symbol::Trailer(_) if high => return self.finish(Outcome::BadEnd),
                Symbol::Protected(level) if level != high => {
                    return self.finish(if high {
                        Outcome::ArbitrationLost
                    } else {
                        Outcome::EchoMismatch
                    });
                }
                _ => {}
            }
            self.phase = Phase::Boundary;
            return Action::None;
        }
        self.symbol = match self.symbol {
            Symbol::Protected(_) => self
                .bits
                .next()
                .map_or(Symbol::Trailer(0), Symbol::Protected),
            Symbol::Trailer(9) => {
                return self.finish(if self.ack {
                    Outcome::Ack
                } else {
                    Outcome::NoAck
                });
            }
            Symbol::Trailer(n) => Symbol::Trailer(n + 1),
        };
        self.phase = Phase::Sample;
        Action::Drive(matches!(self.symbol, Symbol::Protected(true)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const GOLDEN: &[u8] = b"10001001000110000010000010000010000110100101000111000101111";
    fn request() -> Request {
        let mut req = Request {
            port: 0,
            len: 5,
            data: [0; 67],
        };
        req.data[..5].copy_from_slice(&[0x12, 0x30, 0, 1, 0xa5]);
        req
    }

    #[test]
    fn validates_link_structure_only() {
        assert_eq!(Request::new(0, &[0x12, 0x30, 0, 1, 0xa5]), Some(request()));
        assert!(Request::new(5, &[0; 4]).is_none());
        assert!(Request::new(0, &[0; 3]).is_none());
        assert!(Request::new(0, &[0x12, 0x30, 0, 2, 0xa5]).is_none());
    }

    #[test]
    fn streaming_bits_recover_every_body_and_zero_crc_remainder() {
        for len in 0..=63 {
            for fill in [0x00, 0xff, 0x55, 0xf8, 0x1f] {
                let mut body = [fill; 67];
                body[3] = (fill & 0xc0) | len as u8;
                let body = &body[..4 + len];
                let request = Request::new(4, body).unwrap();
                let mut bits = Protected::new(&request);
                let (mut run, mut last, mut remainder) = (1, true, 0x4599u16);
                for i in 0..(body.len() * 8 + 15) {
                    if run == 5 {
                        assert_eq!(bits.next(), Some(!last));
                        last = !last;
                        run = 1;
                    }
                    let bit = bits.next().unwrap();
                    if i / 8 < body.len() {
                        assert_eq!(u8::from(bit), body[i / 8] >> (7 - i % 8) & 1);
                    }
                    remainder = (remainder << 1) ^ (u16::from(bit) << 15);
                    if remainder & 0x8000 != 0 {
                        remainder ^= 0xc599;
                    }
                    run = if last == bit { run + 1 } else { 1 };
                    last = bit;
                }
                assert_eq!(remainder, 0);
                assert_eq!(bits.next(), None);
            }
        }
    }

    #[test]
    fn one_shot_with_and_without_peer_ack() {
        for ack in [true, false] {
            let mut s = Sender::new(&request(), u32::MAX - 800);
            assert_eq!(s.poll(20_000, false), Action::None);
            let mut now = 60_000;
            for (i, b) in GOLDEN.iter().copied().chain(*b"0000000000").enumerate() {
                let level = b == b'1';
                assert_eq!(s.poll(now, false), Action::Drive(level));
                assert_eq!(
                    s.poll(now + 1250, level || (ack && i == GOLDEN.len() + 1)),
                    Action::None
                );
                now += 2500;
            }
            assert_eq!(
                s.poll(now, false),
                Action::Finish(if ack { Outcome::Ack } else { Outcome::NoAck })
            );
            assert_eq!(s.poll(now + 10_000, false), Action::None);
            assert!(!s.retry_arbitration(now + 10_000));
        }
    }

    #[test]
    fn stops_on_late_edges_or_bus_disagreement() {
        for (time, level, out) in [
            (41_250, false, Outcome::EchoMismatch),
            (42_000, true, Outcome::Late),
        ] {
            let mut s = Sender::new(&request(), 0);
            assert_eq!(s.poll(40_000, false), Action::Drive(true));
            assert_eq!(s.poll(time, level), Action::Finish(out));
        }
        let mut s = Sender::new(&request(), 0);
        for (t, level) in [(40_000, false), (41_250, true), (42_500, true)] {
            s.poll(t, level);
        }
        assert_eq!(
            s.poll(43_750, true),
            Action::Finish(Outcome::ArbitrationLost)
        );
        assert!(s.started());
        assert_eq!(s.poll(100_000, false), Action::None); // Raw stays terminal.
        assert!(s.retry_arbitration(100_000));
        assert!(!s.started() && !s.retry_arbitration(110_000));
        assert!(s.hold_if_waiting(200_000));
        assert_eq!(s.poll(230_000, false), Action::None);
        for (i, &b) in GOLDEN.iter().enumerate() {
            let at = 240_000 + (i as u32) * BIT_US;
            assert_eq!(s.poll(at, false), Action::Drive(b == b'1'));
            assert!(!s.hold_if_waiting(at));
            assert_eq!(s.poll(at + BIT_US / 2, b == b'1'), Action::None);
        }
        let mut s = Sender::new(&request(), 0);
        assert_eq!(s.poll(5_000_000, true), Action::None);
    }
}
