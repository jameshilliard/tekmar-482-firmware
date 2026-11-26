//! Listen-only tN4 link decoder. Logic high is dominant; idle is low.

mod response;
pub mod tx;
pub use response::LinkResponse;

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
    pub early_start: bool,
}

pub struct InvalidFrame<'a> {
    pub reason: u8,
    pub bits: u16,
    pub bytes: &'a [u8],
    pub next_bit: u32,
}

#[derive(Clone, Copy)]
enum State {
    Seek(u8),
    Dominant(u8),
    Idle,
    Gap(u8),
    ErrorFlag(u32),
    ErrorDelimiter(i8),
    Data,
    Check(u8),
    Delimiter,
    Ack(u32),
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
    capture: [u8; MAX_BYTES + 2],
    captured: usize,
    failure: Option<(u8, u32)>,
    gap_fault: Option<u32>,
    verify_drive: bool,
    drive_fault: Option<u32>,
    early_start: bool,
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
            capture: [0; MAX_BYTES + 2],
            captured: 0,
            failure: None,
            gap_fault: None,
            verify_drive: false,
            drive_fault: None,
            early_start: false,
            ack: false,
            start: 0,
            level: false,
            edge_us: 0,
            sample_us: BIT_US / 2,
        }
    }
}

impl Receiver {
    pub fn take_failure(&mut self) -> Option<InvalidFrame<'_>> {
        let (reason, next_bit) = self.failure.take()?;
        Some(InvalidFrame {
            reason,
            bits: self.captured as u16,
            bytes: &self.capture[..self.captured.div_ceil(8)],
            next_bit,
        })
    }

    /// The physical slot boundary, fixed when the verified CRC delimiter was sampled.
    pub fn ack_slot(&self) -> Option<u32> {
        match self.state {
            State::Ack(at) => Some(at),
            _ => None,
        }
    }
    pub fn starting_at(high: bool) -> Self {
        Self {
            level: high,
            ..Self::default()
        }
    }

    pub fn partial(&self) -> bool {
        !matches!(
            self.state,
            State::Seek(_)
                | State::Dominant(_)
                | State::Idle
                | State::Gap(_)
                | State::ErrorFlag(_)
                | State::ErrorDelimiter(_)
        )
    }

    pub fn idle(&self) -> bool {
        matches!(self.state, State::Idle) && !self.level
    }

    pub fn take_gap_fault(&mut self) -> Option<u32> {
        self.gap_fault.take()
    }

    /// Arm only after the Normal-mode link response actually claimed and drove the pin.
    pub fn verify_response_drive(&mut self) {
        self.verify_drive = true;
    }

    pub fn take_drive_fault(&mut self) -> Option<u32> {
        self.drive_fault.take()
    }

    pub fn recovering(&self) -> bool {
        matches!(self.state, State::ErrorFlag(_) | State::ErrorDelimiter(_))
    }

    pub fn early_start(&self) -> bool {
        self.early_start
    }

    pub fn stuck_dominant(&self) -> bool {
        matches!(self.state, State::Dominant(14) | State::ErrorDelimiter(14))
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
                    early_start: self.early_start,
                });
            }
        }
    }

    fn bit(&mut self, high: bool) -> Option<Trailer> {
        let b = u8::from(high);
        // advance() has already moved the sample clock to the following bit center.
        let next_bit = self.sample_us.wrapping_sub(BIT_US / 2);
        if matches!(self.state, State::Data | State::Check(_)) {
            self.history = ((self.history << 1) | b) & 63;
            match self.history {
                0 | 63 => {
                    self.failure = Some((1, next_bit));
                    self.stats.stuffing = self.stats.stuffing.saturating_add(1);
                    self.state = State::Seek(0);
                    return None;
                }
                1 | 62 => return None, // complementary stuff bit after five equal bits
                _ => {}
            }
            let offset = self.captured % 8;
            let slot = &mut self.capture[self.captured / 8];
            if offset == 0 {
                *slot = 0;
            }
            *slot |= b << (7 - offset);
            self.captured += 1;
        }
        self.state = match self.state {
            State::Seek(_) if high => State::Dominant(1),
            State::Dominant(n) if high => State::Dominant((n + 1).min(14)),
            State::Dominant(_) => State::Seek(1),
            State::Seek(6) => State::Idle,
            State::Seek(n) => State::Seek(n + 1),
            State::ErrorFlag(end)
                if (self.sample_us.wrapping_sub(BIT_US).wrapping_sub(end) as i32) < 0 =>
            {
                if self.verify_drive && !high {
                    self.drive_fault = Some(next_bit);
                    self.verify_drive = false;
                }
                self.state
            }
            State::ErrorFlag(_) => {
                self.verify_drive = false;
                State::ErrorDelimiter(if high { 1 } else { -1 })
            }
            State::ErrorDelimiter(n) if high => State::ErrorDelimiter((n.max(0) + 1).min(14)),
            State::ErrorDelimiter(-7) => State::Gap(3),
            State::ErrorDelimiter(n) => State::ErrorDelimiter(n.min(0) - 1),
            State::Gap(1) | State::Idle if !high => State::Idle,
            State::Gap(n) if !high => State::Gap(n - 1),
            State::Gap(_) | State::Idle => {
                self.early_start = matches!(self.state, State::Gap(3));
                if self.early_start {
                    self.gap_fault = Some(next_bit);
                }
                self.capture[0] = 0x80;
                self.captured = 1;
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
                    self.failure = Some((2, next_bit));
                    State::Seek(0)
                }
            }
            State::Delimiter if high => {
                self.failure = Some((3, next_bit));
                self.state = State::Seek(0);
                return Some(Trailer::BadDelimiter);
            }
            State::Delimiter => State::Ack(next_bit),
            State::Ack(_) => {
                if core::mem::take(&mut self.verify_drive) && !high {
                    self.drive_fault = Some(next_bit);
                    self.state = State::Seek(0);
                    return None;
                }
                self.ack = high;
                State::End(8)
            }
            State::End(_) if high => {
                self.failure = Some((4, next_bit));
                self.state = State::Seek(0);
                return Some(Trailer::BadEnd { ack: self.ack });
            }
            State::End(1) => {
                self.state = State::Gap(3);
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
    fn fourteen_dominant_idle_samples_signal_a_line_fault_not_a_frame() {
        let mut rx = Receiver::default();
        for _ in 0..13 {
            assert!(rx.bit(true).is_none());
            assert!(!rx.stuck_dominant() && !rx.partial());
        }
        rx.bit(true);
        assert!(rx.stuck_dominant());
        assert!(rx.take_failure().is_none());
        for _ in 0..7 {
            rx.bit(false);
            assert!(!rx.stuck_dominant());
        }
        rx.bit(true);
        assert!(rx.partial());
    }

    #[test]
    fn intermission_is_not_idle_and_only_the_first_gap_bit_flags_backoff() {
        for quiet in 0..=3 {
            let mut rx = Receiver::default();
            for &b in b"0000000".iter().chain(BITS).chain(b"0100000000") {
                rx.bit(b == b'1');
            }
            for _ in 0..quiet {
                assert!(!rx.idle());
                rx.bit(false);
            }
            assert_eq!(rx.idle(), quiet == 3);
            rx.bit(true);
            assert!(rx.partial());
            assert_eq!(rx.early_start(), quiet == 0);
            assert_eq!(rx.take_gap_fault().is_some(), quiet == 0);
            assert!(rx.take_gap_fault().is_none());
        }
    }

    #[test]
    fn ack_slot_is_only_offered_after_a_valid_delimiter_and_crc() {
        for (damage, delimiter, valid) in [
            (false, false, true),
            (true, false, false),
            (false, true, false),
        ] {
            let mut rx = Receiver::default();
            let mut bits = BITS.to_vec();
            if damage {
                *bits.last_mut().unwrap() ^= 1;
            }
            for &b in b"0000000".iter().chain(&bits) {
                rx.bit(b == b'1');
                assert!(rx.ack_slot().is_none());
            }
            rx.sample_us = 21_250;
            rx.bit(delimiter);
            assert_eq!(rx.ack_slot(), valid.then_some(20_000));
            rx.bit(false);
            assert!(rx.ack_slot().is_none());
        }
    }

    #[test]
    fn ack_pulse_is_one_slot_even_across_wrap() {
        let mut ack = LinkResponse::default();
        let at = u32::MAX - 300;
        let slot = Some(at);
        assert_eq!(ack.poll(at - 1, slot), None);
        assert_eq!(ack.poll(at + 5, slot), Some(true));
        assert_eq!(ack.poll(at.wrapping_add(BIT_US - 1), None), None);
        assert_eq!(ack.poll(at.wrapping_add(BIT_US), None), Some(false));
        assert_eq!(ack.poll(at.wrapping_add(BIT_US + 1), slot), None);
        assert_eq!(ack.poll(20_700, Some(20_000)), None);
        assert_eq!(ack.poll(20_701, Some(20_000)), None);
        assert!(!ack.busy());
    }

    #[test]
    fn normal_flag_wraps_once_then_requires_eight_consecutive_quiet_bits_and_gap() {
        let mut rx = Receiver::default();
        let mut response = LinkResponse::default();
        let at = u32::MAX - 4000;
        let end = at.wrapping_add(6 * BIT_US);
        response.recover(&mut rx, at);
        assert!(response.busy() && rx.recovering() && !rx.idle());
        assert_eq!(response.poll(at - 1, Some(at - 1)), None);
        assert_eq!(response.poll(at + 10, None), Some(true));
        response.recover(&mut rx, at + 200); // Must not lengthen the flag.
        rx.verify_response_drive();
        rx.sample_us = at.wrapping_add(BIT_US / 2);
        let tick = |rx: &mut Receiver, high: bool| {
            rx.level = high;
            rx.advance(rx.sample_us, |_| {
                panic!("recovery emitted an application frame")
            });
        };
        for _ in 0..6 {
            tick(&mut rx, true);
        }
        assert!(rx.take_drive_fault().is_none());
        assert_eq!(response.poll(end - 1, None), None);
        assert_eq!(response.poll(end, None), Some(false));
        for _ in 0..7 {
            tick(&mut rx, false);
        }
        for _ in 0..14 {
            tick(&mut rx, true); // Overlapping peer flag restarts the delimiter, not our output.
        }
        assert!(rx.stuck_dominant() && !response.busy());
        for _ in 0..7 {
            tick(&mut rx, false);
            assert!(rx.recovering());
        }
        tick(&mut rx, false);
        for _ in 0..3 {
            assert!(!rx.recovering() && !rx.idle());
            tick(&mut rx, false);
        }
        assert!(rx.idle() && rx.take_failure().is_none() && rx.stats.starts == 0);
        tick(&mut rx, true);
        assert!(rx.partial() && !rx.early_start());
    }

    #[test]
    fn normal_drive_failure_is_distinct_from_listening_to_a_missing_ack() {
        let mut rx = Receiver {
            state: State::Ack(10_000),
            sample_us: 13_750,
            ..Receiver::default()
        };
        rx.verify_response_drive();
        rx.bit(false);
        assert_eq!(rx.take_drive_fault(), Some(12_500));
        assert!(rx.take_failure().is_none());
        let mut response = LinkResponse::default();
        response.recover(&mut rx, 12_500);
        assert_eq!(response.poll(12_500, None), Some(true));
        rx.verify_response_drive();
        rx.sample_us = 16_250;
        rx.bit(false);
        assert_eq!(rx.take_drive_fault(), Some(15_000));
        response.recover(&mut rx, 15_000); // A broken output cannot perpetuate recovery.
        rx.bit(false);
        assert!(rx.take_drive_fault().is_none());
        assert_eq!(response.poll(27_500, None), Some(false));
        assert!(!response.busy());
        rx.state = State::Ack(32_000);
        rx.bit(false);
        assert!(rx.take_drive_fault().is_none());
    }

    #[test]
    fn faults_preserve_raw_evidence_and_late_flags_are_never_driven() {
        for (reason, damaged) in [
            (1, b"111111".to_vec()),
            (2, [BITS[..BITS.len() - 1].to_vec(), b"0".to_vec()].concat()),
            (3, [BITS, b"1"].concat()),
            (4, [BITS, b"001"].concat()),
            (4, [BITS, b"010001"].concat()),
        ] {
            let mut rx = Receiver {
                state: State::Idle,
                ..Receiver::default()
            };
            for &b in &damaged {
                rx.sample_us += BIT_US;
                rx.bit(b == b'1');
            }
            let expected = rx.sample_us - BIT_US / 2;
            let fault = rx.take_failure().unwrap();
            assert_eq!((fault.reason, fault.next_bit), (reason, expected));
            assert!(!fault.bytes.is_empty() && !rx.recovering());
            let mut response = LinkResponse::default();
            response.recover(&mut rx, expected);
            assert_eq!(response.poll(expected + BIT_US / 4, None), None);
            assert_eq!(response.poll(expected + BIT_US, None), None);
            assert!(!response.busy());
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
