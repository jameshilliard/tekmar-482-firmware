//! Boot-lifetime counters and a bounded, fair mailbox for opt-in error reports.
use crate::tn4::{PORT_COUNT, tx::Outcome};

pub const GATEWAY: u8 = PORT_COUNT as u8;
pub const ALL_SOURCES: u8 = (1 << (PORT_COUNT + 1)) - 1;
pub const REPORT_BYTES: usize = 2 + size_of::<u32>() + size_of::<u64>();

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum LineError {
    Stuffing = 1,
    Crc,
    CrcDelimiter,
    BadEnd,
    RxNoAck,
    EarlyStart,
    StuckDominant,
    ResponseDrive,
    TxNoAck,
    ArbitrationLost,
    TxEchoMismatch,
    TxBadEnd,
    TxMissedTiming,
    PinUnavailable,
    RawValidDrop,
    RawInvalidDrop,
    TraceDrop,
    StatusReadDrop,
}

impl LineError {
    pub fn from_outcome(outcome: Outcome) -> Option<Self> {
        Some(match outcome {
            Outcome::Ack => return None,
            Outcome::NoAck => Self::TxNoAck,
            Outcome::ArbitrationLost => Self::ArbitrationLost,
            Outcome::EchoMismatch => Self::TxEchoMismatch,
            Outcome::BadEnd => Self::TxBadEnd,
            Outcome::Late => Self::TxMissedTiming,
        })
    }
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum HostError {
    UartOverrun = 0x80,
    UartParity,
    UartFraming,
    UartBreak,
    TpckChecksum,
    TpckFraming,
    TpckResync,
    AddressNoticeDrop,
}

const LINE_KEYS: usize = LineError::StatusReadDrop as usize;
const HOST_KEYS: usize =
    HostError::AddressNoticeDrop as usize - HostError::UartOverrun as usize + 1;
const KEYS: usize = PORT_COUNT * LINE_KEYS + HOST_KEYS;

fn index(source: u8, code: u8) -> Option<usize> {
    if source < GATEWAY && (1..=LINE_KEYS as u8).contains(&code) {
        Some(source as usize * LINE_KEYS + usize::from(code - 1))
    } else if source == GATEWAY
        && (HostError::UartOverrun as u8..=HostError::AddressNoticeDrop as u8).contains(&code)
    {
        Some(PORT_COUNT * LINE_KEYS + usize::from(code - HostError::UartOverrun as u8))
    } else {
        None
    }
}

fn key(i: usize) -> (u8, u8) {
    if i < PORT_COUNT * LINE_KEYS {
        ((i / LINE_KEYS) as u8, (i % LINE_KEYS) as u8 + 1)
    } else {
        (
            GATEWAY,
            (i - PORT_COUNT * LINE_KEYS) as u8 + HostError::UartOverrun as u8,
        )
    }
}

pub struct ErrorBook {
    counts: [u64; KEYS],
    seen_ms: [u32; KEYS],
    dirty: [bool; KEYS],
    pending: usize,
    cursor: usize,
    mask: u8,
}

impl ErrorBook {
    pub const EMPTY: Self = Self {
        counts: [0; KEYS],
        seen_ms: [0; KEYS],
        dirty: [false; KEYS],
        pending: 0,
        cursor: 0,
        mask: 0,
    };

    pub fn mask(&self) -> u8 {
        self.mask
    }

    pub fn set_mask(&mut self, mask: u8) {
        self.mask = mask;
        for i in 0..KEYS {
            if self.dirty[i] && mask & (1 << key(i).0) == 0 {
                self.dirty[i] = false;
                self.pending -= 1;
            }
        }
    }

    fn observe(&mut self, source: u8, i: usize, ms: u32) {
        self.counts[i] = self.counts[i].saturating_add(1);
        self.seen_ms[i] = ms;
        if self.mask & (1 << source) != 0 && !self.dirty[i] {
            self.dirty[i] = true;
            self.pending += 1;
        }
    }

    pub fn line(&mut self, port: usize, code: LineError, ms: u32) {
        self.observe(port as u8, port * LINE_KEYS + code as usize - 1, ms);
    }

    pub fn host(&mut self, code: HostError, ms: u32) {
        self.observe(
            GATEWAY,
            PORT_COUNT * LINE_KEYS + (code as usize - HostError::UartOverrun as usize),
            ms,
        );
    }

    pub fn count(&self, port: usize, code: LineError) -> u64 {
        self.counts[port * LINE_KEYS + code as usize - 1]
    }

    fn encode(&self, i: usize) -> [u8; REPORT_BYTES] {
        let (source, code) = key(i);
        super::pack(&[
            &[source, code],
            &self.seen_ms[i].to_le_bytes(),
            &self.counts[i].to_le_bytes(),
        ])
    }

    pub fn read(&self, source: u8, code: u8) -> Option<[u8; REPORT_BYTES]> {
        index(source, code).map(|i| self.encode(i))
    }

    pub fn next_report(&mut self) -> Option<[u8; REPORT_BYTES]> {
        if self.pending == 0 {
            return None;
        }
        loop {
            let i = self.cursor;
            self.cursor = (i + 1) % KEYS;
            if core::mem::take(&mut self.dirty[i]) {
                self.pending -= 1;
                return Some(self.encode(i));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_history_coalescing_mask_saturation_and_fairness() {
        let mut e = ErrorBook::EMPTY;
        e.line(1, LineError::Crc, u32::MAX);
        assert!(e.next_report().is_none());
        assert_eq!(
            e.read(1, 2).unwrap(),
            [1, 2, 255, 255, 255, 255, 1, 0, 0, 0, 0, 0, 0, 0]
        );
        assert!(e.read(5, 2).is_none() && e.read(1, 0x80).is_none() && e.read(6, 1).is_none());
        e.line(4, LineError::StatusReadDrop, 10);
        assert_eq!(&e.read(4, 0x12).unwrap()[6..], &1u64.to_le_bytes());
        assert!(e.next_report().is_none());
        e.set_mask(0x23);
        assert!(e.next_report().is_none()); // Enabling is not a replay.
        e.line(1, LineError::Crc, 0);
        e.line(1, LineError::Crc, 1);
        e.host(HostError::UartParity, 2);
        assert_eq!(
            e.next_report().unwrap(),
            [1, 2, 1, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0]
        );
        e.line(1, LineError::Crc, 3);
        assert_eq!(e.next_report().unwrap()[..2], [5, 0x81]);
        e.set_mask(1);
        assert!(e.next_report().is_none());
        e.counts[0] = u64::MAX;
        e.line(0, LineError::Stuffing, 4);
        assert_eq!(e.next_report().unwrap()[6..], [255; 8]);
    }
}
