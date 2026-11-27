//! Boot-lifetime counters and a bounded, fair mailbox for opt-in error reports.
use crate::report_cursor::ReportCursor;
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
    NativeReadDrop,
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

const LINE_KEYS: usize = LineError::NativeReadDrop as usize - LineError::Stuffing as usize + 1;
const HOST_KEYS: usize =
    HostError::AddressNoticeDrop as usize - HostError::UartOverrun as usize + 1;
const SOURCES: usize = PORT_COUNT + 1;
const KEYS: usize = PORT_COUNT * LINE_KEYS + HOST_KEYS;
const _: () = assert!(HOST_KEYS <= LINE_KEYS && LINE_KEYS <= u32::BITS as usize);

fn first_code(source: usize) -> u8 {
    if source == PORT_COUNT {
        HostError::UartOverrun as u8
    } else {
        LineError::Stuffing as u8
    }
}

pub struct ErrorBook {
    counts: [u64; KEYS],
    seen_ms: [u32; KEYS],
    dirty: [u32; SOURCES],
    cursor: ReportCursor,
    mask: u8,
}

impl ErrorBook {
    pub const EMPTY: Self = Self {
        counts: [0; KEYS],
        seen_ms: [0; KEYS],
        dirty: [0; SOURCES],
        cursor: ReportCursor::START,
        mask: 0,
    };

    pub fn mask(&self) -> u8 {
        self.mask
    }

    pub fn set_mask(&mut self, mask: u8) {
        self.mask = mask;
        for (i, dirty) in self.dirty.iter_mut().enumerate() {
            if mask & (1 << i) == 0 {
                *dirty = 0;
            }
        }
    }

    fn observe(&mut self, source: usize, offset: usize, ms: u32) {
        let i = source * LINE_KEYS + offset;
        self.counts[i] = self.counts[i].saturating_add(1);
        self.seen_ms[i] = ms;
        if self.mask & (1 << source) != 0 {
            self.dirty[source] |= 1 << offset;
        }
    }

    pub fn line(&mut self, port: usize, code: LineError, ms: u32) {
        self.observe(port, code as usize - LineError::Stuffing as usize, ms);
    }

    pub fn host(&mut self, code: HostError, ms: u32) {
        self.observe(
            PORT_COUNT,
            code as usize - HostError::UartOverrun as usize,
            ms,
        );
    }

    pub fn count(&self, port: usize, code: LineError) -> u64 {
        self.counts[port * LINE_KEYS + code as usize - LineError::Stuffing as usize]
    }

    fn encode(&self, source: usize, offset: usize) -> [u8; REPORT_BYTES] {
        let i = source * LINE_KEYS + offset;
        super::pack(&[
            &[source as u8, first_code(source) + offset as u8],
            &self.seen_ms[i].to_le_bytes(),
            &self.counts[i].to_le_bytes(),
        ])
    }

    pub fn read(&self, source: u8, code: u8) -> Option<[u8; REPORT_BYTES]> {
        let source = usize::from(source);
        let keys = match source {
            ..PORT_COUNT => LINE_KEYS,
            PORT_COUNT => HOST_KEYS,
            _ => return None,
        };
        let offset = usize::from(code.checked_sub(first_code(source))?);
        (offset < keys).then(|| self.encode(source, offset))
    }

    pub fn next_report(&mut self) -> Option<[u8; REPORT_BYTES]> {
        let (source, offset) = self.cursor.select(&self.dirty, Some)?;
        self.dirty[source] &= !(1 << offset);
        Some(self.encode(source, offset))
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
        e.line(4, LineError::NativeReadDrop, 10);
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

    #[test]
    fn sparse_counters_rotate_across_sources_without_resetting_on_mask_changes() {
        let mut e = ErrorBook::EMPTY;
        e.set_mask(ALL_SOURCES);
        e.line(0, LineError::Stuffing, 1);
        e.line(0, LineError::NativeReadDrop, 2);
        e.line(4, LineError::TxMissedTiming, 3);
        e.host(HostError::AddressNoticeDrop, 4);
        assert_eq!(e.next_report().unwrap()[..2], [0, 1]);
        e.line(0, LineError::Stuffing, 5);
        assert_eq!(e.next_report().unwrap()[..2], [0, 18]);
        assert_eq!(e.next_report().unwrap()[..2], [4, 13]);
        e.set_mask(ALL_SOURCES & !(1 << GATEWAY));
        e.set_mask(ALL_SOURCES);
        assert_eq!(
            e.next_report().unwrap(),
            [0, 1, 5, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0],
        );
        assert!(e.next_report().is_none());
        assert_eq!(
            &e.read(GATEWAY, 0x87).unwrap()[2..],
            &[4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        );
    }
}
