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
    records: [Record; 13],
    order: [u8; 13],
    used: u16,
    len: usize,
    offset: usize,
    pub(super) drops: [[u32; 2]; 5],
}

impl Output {
    pub(super) fn take() -> &'static mut Self {
        #[unsafe(link_section = ".low_bss")]
        static QUEUE: static_cell::ConstStaticCell<Output> =
            static_cell::ConstStaticCell::new(Output {
                records: [const {
                    Record {
                        bytes: Vec::new(),
                        kind: Kind::Rx(0),
                    }
                }; 13],
                order: [0; 13],
                used: 0,
                len: 0,
                offset: 0,
                drops: [[0; 2]; 5],
            });
        QUEUE.take()
    }
    pub(super) fn can_report(&self) -> bool {
        self.len < 2
    }
    pub(super) fn can_receive(&self) -> bool {
        self.len < 8
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
    fn remove(&mut self, pos: usize) -> Kind {
        let slot = self.order[pos];
        self.used &= !(1 << slot);
        self.len -= 1;
        for i in pos..self.len {
            self.order.swap(i, i + 1);
        }
        self.records[usize::from(slot)].kind
    }
    pub(super) fn enqueue(&mut self, kind: Kind, ty: u8, data: &[u8]) {
        let first = usize::from(self.offset != 0);
        if matches!(kind, Kind::Rx(_) | Kind::Invalid(_)) && self.len >= 6 {
            if matches!(kind, Kind::Rx(_))
                && let Some(i) = self.order[first..self.len]
                    .iter()
                    .position(|&s| matches!(self.records[usize::from(s)].kind, Kind::Invalid(_)))
            {
                let old = self.remove(first + i);
                self.lost(old);
            } else {
                self.lost(kind);
                return;
            }
        }
        let pos = if let Kind::Overlap(port) = kind {
            self.order[first..self.len]
                .iter()
                .position(|&s| self.records[usize::from(s)].kind == Kind::Result(port))
                .map(|n| first + n)
        } else {
            None
        };
        let slot = (!self.used).trailing_zeros() as usize;
        self.used |= 1 << slot;
        let r = &mut self.records[slot];
        r.kind = kind;
        r.bytes.clear();
        tpck::encode(ty, data, |b| r.bytes.push(b).unwrap()).unwrap();
        self.order[self.len] = slot as u8;
        for i in (pos.unwrap_or(self.len)..self.len).rev() {
            self.order.swap(i, i + 1);
        }
        self.len += 1;
    }
    pub(super) fn pump(&mut self) -> Option<Kind> {
        if self.len == 0 {
            return None;
        }
        let r = &self.records[usize::from(self.order[0])];
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
        Some(self.remove(0))
    }
}
