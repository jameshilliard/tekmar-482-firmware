//! One bus attempt. No automatic retransmission or active error flags.
use super::BIT_US;

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub port: u8,
    data: [u8; 67],
    len: usize,
}

impl Request {
    pub fn parse(line: &[u8]) -> Option<Self> {
        // Protect the selected physical port as well as the bus header and payload.
        let [port @ b'0'..=b'4', b' ', tail @ ..] = line else {
            return None;
        };
        let space = tail.iter().position(|&b| b == b' ')?;
        let (body, check) = tail.split_at(space);
        if !(8..=134).contains(&space) || space % 2 != 0 || check.len() != 5 {
            return None;
        }
        let mut request = Self {
            port: port - b'0',
            len: space / 2,
            data: [0; 67],
        };
        for (to, pair) in request.data.iter_mut().zip(body.as_chunks::<2>().0) {
            *to = hex(pair[0])? << 4 | hex(pair[1])?;
        }
        let crc = check[1..]
            .iter()
            .try_fold(0u16, |n, &b| Some(n << 4 | u16::from(hex(b)?)))?;
        let mut actual = 0xffffu16;
        for b in core::iter::once(&request.port).chain(request.body()) {
            actual ^= u16::from(*b) << 8;
            for _ in 0..8 {
                actual = (actual << 1) ^ if actual & 0x8000 != 0 { 0x1021 } else { 0 };
            }
        }
        (actual == crc && usize::from(request.data[3] & 63) + 4 == request.len).then_some(request)
    }

    fn body(&self) -> &[u8] {
        &self.data[..self.len]
    }
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

struct Bits {
    // At most 552 unstuffed bits plus one added per four bits.
    bytes: [u8; 87],
    len: usize,
}

impl Bits {
    fn push(&mut self, bit: bool) {
        if bit {
            self.bytes[self.len / 8] |= 1 << (self.len % 8);
        }
        self.len += 1;
    }
    fn encode(body: &[u8]) -> Self {
        let mut bits = Self {
            bytes: [0; 87],
            len: 0,
        };
        let mut run = 1;
        let mut last = true;
        bits.push(true);
        let mut put = |bit| {
            if run == 5 {
                last = !last;
                bits.push(last);
                run = 1;
            }
            run = if last == bit { run + 1 } else { 1 };
            last = bit;
            bits.push(bit);
        };
        let mut crc = 0x4599u16;
        for &byte in body {
            for shift in (0..8).rev() {
                let bit = byte & (1 << shift) != 0;
                let feedback = (crc & 0x4000 != 0) ^ bit;
                crc = ((crc << 1) & 0x7fff) ^ if feedback { 0x4599 } else { 0 };
                put(bit);
            }
        }
        for shift in (0..15).rev() {
            put(crc & (1 << shift) != 0);
        }
        bits
    }
    fn level(&self, index: usize) -> bool {
        index < self.len && self.bytes[index / 8] & (1 << (index % 8)) != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ack,
    NoAck,
    ArbitrationLost,
    EchoMismatch,
    BadEnd,
    Busy,
    Late,
}

impl Outcome {
    pub fn label(self) -> &'static [u8] {
        match self {
            Self::Ack => b"ACK",
            Self::NoAck => b"NOACK",
            Self::ArbitrationLost => b"ARBITRATION",
            Self::EchoMismatch => b"ECHO",
            Self::BadEnd => b"BAD-END",
            Self::Busy => b"BUSY",
            Self::Late => b"LATE",
        }
    }
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
    Done,
}

pub struct Sender {
    bits: Bits,
    phase: Phase,
    submitted: u32,
    quiet_since: u32,
    due: u32,
    index: usize,
    ack: bool,
}

impl Sender {
    pub fn new(request: &Request, now: u32) -> Self {
        Self {
            bits: Bits::encode(request.body()),
            phase: Phase::Idle,
            submitted: now,
            quiet_since: now,
            due: now,
            index: 0,
            ack: false,
        }
    }

    fn finish(&mut self, outcome: Outcome) -> Action {
        self.phase = Phase::Done;
        Action::Finish(outcome)
    }

    pub fn poll(&mut self, now: u32, high: bool) -> Action {
        match self.phase {
            Phase::Done => return Action::None,
            Phase::Idle => {
                if high {
                    self.quiet_since = now;
                }
                if now.wrapping_sub(self.submitted) >= 5_000_000 {
                    return self.finish(Outcome::Busy);
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
            if self.index == self.bits.len + 1 {
                self.ack = high;
            } else if self.bits.level(self.index) != high {
                let outcome = if self.index >= self.bits.len {
                    Outcome::BadEnd
                } else if high {
                    Outcome::ArbitrationLost
                } else {
                    Outcome::EchoMismatch
                };
                return self.finish(outcome);
            }
            self.phase = Phase::Boundary;
            return Action::None;
        }
        self.index += 1;
        if self.index == self.bits.len + 10 {
            return self.finish(if self.ack {
                Outcome::Ack
            } else {
                Outcome::NoAck
            });
        }
        self.phase = Phase::Sample;
        Action::Drive(self.bits.level(self.index))
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
    fn validates_command_before_claiming_a_port() {
        assert_eq!(Request::parse(b"0 12300001a5 b45e"), Some(request()));
        assert_eq!(Request::parse(b"4 12300001A5 B2FF").unwrap().port, 4);
        for invalid in [
            b"1 12300001a5 b45e".as_slice(), // CRC includes the port
            b"0 12300002a5 e10d",            // valid CRC, invalid embedded length
            b"0 12300001a4 b45e",
            b"0 12300001a5 b45e1",
            b"5 12300001a5 b45e",
            b"0 12300001ag b45e",
        ] {
            assert!(Request::parse(invalid).is_none());
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
        let mut s = Sender::new(&request(), 0);
        assert_eq!(s.poll(5_000_000, true), Action::Finish(Outcome::Busy));
    }
}
