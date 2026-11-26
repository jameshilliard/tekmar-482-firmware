//! Bounded binary UART records. Only an unstarted record may be displaced.
use super::{LSR, TEMT, THRE, UART0, read, write};
use heapless::Vec;
use tekmar_482_firmware::tpck;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Rx(u8),
    Invalid(u8),
    Result(u8),
    Overlap(u8),
    Control,
    Reboot,
}

struct Record {
    bytes: Vec<u8, 168>,
    kind: Kind,
}

pub(super) struct Output {
    records: Vec<Record, 13>,
    offset: usize,
    pub(super) drops: [[u32; 2]; 5],
}

impl Output {
    pub(super) fn new() -> Self {
        Self {
            records: Vec::new(),
            offset: 0,
            drops: [[0; 2]; 5],
        }
    }
    pub(super) fn can_receive(&self) -> bool {
        self.records.len() < 8
    }
    fn lost(&mut self, kind: Kind) {
        let (p, i) = match kind {
            Kind::Rx(p) => (p, 0),
            Kind::Invalid(p) => (p, 1),
            _ => unreachable!(),
        };
        let n = &mut self.drops[usize::from(p)][i];
        *n = n.wrapping_add(1);
    }
    pub(super) fn enqueue(&mut self, kind: Kind, ty: u8, data: &[u8]) {
        let first = usize::from(self.offset != 0);
        if matches!(kind, Kind::Rx(_) | Kind::Invalid(_)) && self.records.len() >= 6 {
            if matches!(kind, Kind::Rx(_))
                && let Some(i) = self.records[first..]
                    .iter()
                    .position(|r| matches!(r.kind, Kind::Invalid(_)))
            {
                let old = self.records.remove(first + i);
                self.lost(old.kind);
            } else {
                self.lost(kind);
                return;
            }
        }
        let mut bytes = Vec::new();
        tpck::encode(ty, data, |b| bytes.push(b).unwrap()).unwrap();
        let pos = if let Kind::Overlap(port) = kind {
            self.records[first..]
                .iter()
                .position(|r| r.kind == Kind::Result(port))
                .map(|n| first + n)
        } else {
            None
        };
        self.records
            .insert(pos.unwrap_or(self.records.len()), Record { kind, bytes })
            .ok()
            .unwrap();
    }
    pub(super) fn pump(&mut self) -> Option<Kind> {
        let r = self.records.first()?;
        if self.offset < r.bytes.len() {
            if read(LSR) & THRE == 0 {
                return None;
            }
            let end = (self.offset + 16).min(r.bytes.len());
            for &byte in &r.bytes[self.offset..end] {
                write(UART0, u32::from(byte));
            }
            self.offset = end;
        }
        if self.offset != r.bytes.len() {
            return None;
        }
        if matches!(r.kind, Kind::Result(_) | Kind::Reboot) && read(LSR) & TEMT == 0 {
            return None;
        }
        self.offset = 0;
        Some(self.records.remove(0).kind)
    }
}
