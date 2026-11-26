//! Listen-only tN4 link decoder. Logic high is dominant; idle is low.

pub mod tx;

const BIT_US: u32 = 2500;
const MAX_BYTES: usize = 4 + 63;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trailer {
    Ack,
    NoAck,
    BadDelimiter,
    BadEnd { ack: bool },
}

pub struct Frame<'a> {
    pub start_us: u32,
    pub bytes: &'a [u8],
    pub crc: u16,
    pub trailer: Trailer,
}

#[derive(Clone, Copy)]
enum State {
    Seek(u8),
    Idle,
    Data,
    Check(u8),
    Delimiter,
    Ack,
    End(u8),
}

pub struct Receiver {
    pub stats: RxStats,
    state: State,
    bytes: [u8; MAX_BYTES],
    bits: usize,
    history: u8,
    crc: u16,
    received: u16,
    ack: bool,
    start: u32,
    level: bool,
    edge_us: u32,
    sample_us: u32,
}

#[derive(Default)]
pub struct RxStats {
    pub edges: u32,
    pub starts: u32,
    pub stuffing: u32,
    pub crc_mismatch: u32,
    pub ack: u32,
    pub no_ack: u32,
    pub bad_trailer: u32,
    pub last_received_crc: u16,
    pub last_expected_crc: u16,
}

impl Default for Receiver {
    fn default() -> Self {
        Self {
            stats: RxStats::default(),
            state: State::Seek(0),
            bytes: [0; MAX_BYTES],
            bits: 0,
            history: 0,
            crc: 0,
            received: 0,
            ack: false,
            start: 0,
            level: false,
            edge_us: 0,
            sample_us: BIT_US / 2,
        }
    }
}

impl Receiver {
    pub fn starting_at(high: bool) -> Self {
        Self {
            level: high,
            ..Self::default()
        }
    }

    pub fn partial(&self) -> bool {
        !matches!(self.state, State::Seek(_) | State::Idle)
    }

    /// Sample at bit centers, re-centering on each captured transition.
    /// Advance up to the edge with the *previous* level before installing it.
    pub fn edge(&mut self, time: u32, high: bool, emit: impl FnMut(Frame<'_>)) {
        self.stats.edges = self.stats.edges.saturating_add(1);
        self.advance(time, emit);
        self.level = high;
        self.edge_us = time;
        self.sample_us = time.wrapping_add(BIT_US / 2);
    }

    pub fn advance(&mut self, time: u32, mut emit: impl FnMut(Frame<'_>)) {
        while (time.wrapping_sub(self.sample_us) as i32) >= 0 {
            self.sample_us = self.sample_us.wrapping_add(BIT_US);
            if let Some(trailer) = self.bit(self.level) {
                let counter = match trailer {
                    Trailer::Ack => &mut self.stats.ack,
                    Trailer::NoAck => &mut self.stats.no_ack,
                    _ => &mut self.stats.bad_trailer,
                };
                *counter = counter.saturating_add(1);
                emit(Frame {
                    start_us: self.start,
                    bytes: &self.bytes[..self.bits / 8],
                    crc: self.received,
                    trailer,
                });
            }
        }
    }

    fn bit(&mut self, high: bool) -> Option<Trailer> {
        let b = u8::from(high);
        if matches!(self.state, State::Data | State::Check(_)) {
            self.history = ((self.history << 1) | b) & 63;
            match self.history {
                0 | 63 => {
                    self.stats.stuffing = self.stats.stuffing.saturating_add(1);
                    self.state = State::Seek(0);
                    return None;
                }
                1 | 62 => return None, // complementary stuff bit after five equal bits
                _ => {}
            }
        }
        self.state = match self.state {
            State::Seek(_) if high => State::Seek(0),
            State::Seek(6) => State::Idle,
            State::Seek(n) => State::Seek(n + 1),
            State::Idle if !high => State::Idle,
            State::Idle => {
                self.stats.starts = self.stats.starts.saturating_add(1);
                self.start = self.edge_us;
                self.bits = 0;
                self.history = 1; // SOF participates in stuffing and CRC
                self.crc = 0x4599;
                self.received = 0;
                State::Data
            }
            State::Data => {
                let feedback = ((self.crc >> 14) != 0) ^ high;
                self.crc = (self.crc << 1) & 0x7fff;
                if feedback {
                    self.crc ^= 0x4599;
                }
                let byte = &mut self.bytes[self.bits / 8];
                *byte = (*byte << 1) | b;
                self.bits += 1;
                if self.bits >= 32 && self.bits == (4 + usize::from(self.bytes[3] & 63)) * 8 {
                    State::Check(15)
                } else {
                    State::Data
                }
            }
            State::Check(n) => {
                self.received = (self.received << 1) | u16::from(b);
                if n != 1 {
                    State::Check(n - 1)
                } else if self.received == self.crc {
                    State::Delimiter
                } else {
                    self.stats.crc_mismatch = self.stats.crc_mismatch.saturating_add(1);
                    self.stats.last_received_crc = self.received;
                    self.stats.last_expected_crc = self.crc;
                    State::Seek(0)
                }
            }
            State::Delimiter if high => {
                self.state = State::Seek(0);
                return Some(Trailer::BadDelimiter);
            }
            State::Delimiter => State::Ack,
            State::Ack => {
                self.ack = high;
                State::End(8)
            }
            State::End(_) if high => {
                self.state = State::Seek(0);
                return Some(Trailer::BadEnd { ack: self.ack });
            }
            State::End(1) => {
                self.state = State::Idle;
                return Some(if self.ack {
                    Trailer::Ack
                } else {
                    Trailer::NoAck
                });
            }
            State::End(n) => State::End(n - 1), // ACK delimiter then seven low EOF bits
        };
        None
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    // SOF, stuffed 12300001 a5, CRC 0e2f. Tail supplied separately.
    const BITS: &[u8] = b"10001001000110000010000010000010000110100101000111000101111";

    fn feed(bits: &[u8]) -> Vec<(u32, Vec<u8>, u16, Trailer)> {
        let mut r = Receiver::default();
        // Start just before timer wrap and perturb edge timing in each direction.
        let start = u32::MAX - 30_000;
        r.sample_us = start;
        let mut frames = Vec::new();
        let mut emit = |f: Frame<'_>| frames.push((f.start_us, f.bytes.to_vec(), f.crc, f.trailer));
        let mut high = false;
        let mut t = start.wrapping_add(25_000);
        for &bit in bits {
            let level = bit == b'1';
            if high != level {
                r.edge(
                    t.wrapping_add(if level { 130 } else { 20 }),
                    level,
                    &mut emit,
                );
                high = level;
            }
            t = t.wrapping_add(BIT_US);
        }
        r.advance(t, emit);
        frames
    }

    #[test]
    fn checks_crc_and_trailer_independently() {
        for (tail, result) in [
            (b"0100000000".as_slice(), Trailer::Ack),
            (b"0000000000", Trailer::NoAck),
            (b"0011111100", Trailer::BadEnd { ack: false }),
            (b"0101000000", Trailer::BadEnd { ack: true }),
            (b"1000000000", Trailer::BadDelimiter),
        ] {
            let frames = feed(&[BITS, tail].concat());
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].0, u32::MAX - 4870);
            assert_eq!(frames[0].1, [0x12, 0x30, 0, 1, 0xa5]);
            assert_eq!((frames[0].2, frames[0].3), (0xe2f, result));
        }
    }

    #[test]
    fn rejects_bad_crc_and_stuffing_and_resynchronizes() {
        for flip in [3, 17, BITS.len() - 2] {
            let mut bad = BITS.to_vec();
            bad[flip] ^= 1;
            assert!(feed(&[&bad[..], b"0100000000"].concat()).is_empty());
            let both = [&bad[..], b"0100000000000", BITS, b"0100000000"].concat();
            assert_eq!(feed(&both).len(), 1);
        }
        assert!(feed(&BITS[..BITS.len() - 3]).is_empty());
    }

    #[test]
    fn reports_incomplete_stuffed_and_crc_failed_candidates_separately() {
        let mut rx = Receiver::default();
        for &b in b"0000000111111" {
            rx.bit(b == b'1');
        }
        assert_eq!(rx.stats.stuffing, 1);
        assert!(!rx.partial());
        for &b in b"0000000" {
            rx.bit(b == b'1');
        }
        let mut bits = BITS.to_vec();
        *bits.last_mut().unwrap() ^= 1;
        for b in bits {
            rx.bit(b == b'1');
        }
        assert_eq!(rx.stats.starts, 2);
        assert_eq!(rx.stats.crc_mismatch, 1);
        assert_eq!(rx.stats.last_expected_crc, 0xe2f);
        assert_eq!(rx.stats.last_received_crc, 0xe2e);
        for &b in b"0000000110" {
            rx.bit(b == b'1');
        }
        assert!(rx.partial());
        assert_eq!(rx.stats.starts, 3);
    }

    #[test]
    fn full_buffer_and_back_to_back_frames() {
        let ones = [b'1'; 1 + 8 * 67]; // SOF + maximum length all-ff header and payload
        let mut wire = Vec::new();
        let (mut last, mut run) = (0, 0);
        for &bit in ones.iter().chain(b"111101110101011") {
            // CRC 7bab
            if run == 5 {
                last ^= 1;
                wire.push(last);
                run = 1;
            }
            run = if last == bit { run + 1 } else { 1 };
            last = bit;
            wire.push(bit);
        }
        let frames = feed(&[&wire[..], b"0100000000000", BITS, b"0100000000"].concat());
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].1, [0xff; 67]);
        assert_eq!(frames[0].2, 0x7bab);
    }
}
