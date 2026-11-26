//! One bus attempt. No automatic retransmission or active error flags.
use super::BIT_US;

#[derive(Debug, PartialEq, Eq)]
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
    fn body(&self) -> &[u8] {
        &self.data[..self.len]
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
    Done,
}

pub struct Sender {
    bits: Bits,
    phase: Phase,
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
    fn validates_link_structure_only() {
        assert_eq!(Request::new(0, &[0x12, 0x30, 0, 1, 0xa5]), Some(request()));
        assert!(Request::new(5, &[0; 4]).is_none());
        assert!(Request::new(0, &[0; 3]).is_none());
        assert!(Request::new(0, &[0x12, 0x30, 0, 2, 0xa5]).is_none());
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
        assert_eq!(s.poll(5_000_000, true), Action::None);
    }
}
