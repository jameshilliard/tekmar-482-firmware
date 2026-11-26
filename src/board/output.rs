//! Bounded binary UART records. Only an unstarted record may be displaced.
use super::{lpc2146::uart0, read, write};
use tekmar_482_firmware::tn4::{MAX_BYTES, PORT_COUNT};
const REPORT_WATERMARK: usize = 2;
const RX_WATERMARK: usize = 6;
const COMMAND_WATERMARK: usize = 8;
const TRACE_SLOTS: usize = 3;
const RECORD_SLOTS: usize = COMMAND_WATERMARK + PORT_COUNT + TRACE_SLOTS;
const _: () = assert!(RECORD_SLOTS <= u16::BITS as usize);
const ENCODED_RECORD_BYTES: usize = 168; // includes TPCK escaping
use heapless::Vec;
use tekmar_482_firmware::tpck;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Rx(u8),
    Invalid(u8),
    Result(u8),
    Overlap(u8),
    Control,
    Trace,
    Fence,
    Reboot,
}

struct Record {
    bytes: Vec<u8, ENCODED_RECORD_BYTES>,
    kind: Kind,
}

pub(super) struct Output {
    records: [Record; RECORD_SLOTS],
    order: [u8; RECORD_SLOTS],
    used: u16,
    len: usize,
    traces: usize,
    offset: usize,
    pub(super) drops: [[u32; 2]; PORT_COUNT],
    pub(super) trace_drops: [u32; PORT_COUNT],
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
                }; RECORD_SLOTS],
                order: [0; RECORD_SLOTS],
                used: 0,
                len: 0,
                traces: 0,
                offset: 0,
                drops: [[0; 2]; PORT_COUNT],
                trace_drops: [0; PORT_COUNT],
            });
        QUEUE.take()
    }
    pub(super) fn can_report(&self) -> bool {
        self.len - self.traces < REPORT_WATERMARK
    }
    pub(super) fn can_receive(&self) -> bool {
        self.len - self.traces < COMMAND_WATERMARK
    }
    pub(super) fn trace(&mut self, event: u8, p: usize, ms: u32, detail: &[u8], body: &[u8]) {
        if self.traces == TRACE_SLOTS {
            self.trace_drops[p] = self.trace_drops[p].wrapping_add(1);
            return;
        }
        // Invalid frames include a reason, bit count and up to two CRC bytes.
        let mut data = [0; tpck::raw::INVALID_PREFIX + MAX_BYTES + size_of::<u16>()];
        let (prefix, tail) = data.split_at_mut(tpck::raw::RX_PREFIX);
        prefix[0] = event;
        prefix[1] = p as u8;
        prefix[2..].copy_from_slice(&ms.to_le_bytes());
        let (meta, rest) = tail.split_at_mut(detail.len());
        meta.copy_from_slice(detail);
        rest[..body.len()].copy_from_slice(body);
        let len = tpck::raw::RX_PREFIX + detail.len() + body.len();
        self.enqueue(Kind::Trace, tpck::NORMAL_TRACE, &data[..len]);
        self.traces += 1;
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
        let kind = self.records[usize::from(slot)].kind;
        if kind == Kind::Trace {
            self.traces -= 1;
        }
        kind
    }
    pub(super) fn enqueue(&mut self, kind: Kind, ty: u8, data: &[u8]) {
        let first = usize::from(self.offset != 0);
        if matches!(kind, Kind::Rx(_) | Kind::Invalid(_)) && self.len - self.traces >= RX_WATERMARK
        {
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
        } else if kind == Kind::Control && self.traces != 0 {
            // Control may overtake pending diagnostics, but never a partial
            // frame, a trace configuration fence, or an older control record.
            Some(
                self.order[first..self.len]
                    .iter()
                    .rposition(|&s| self.records[usize::from(s)].kind != Kind::Trace)
                    .map_or(first, |i| first + i + 1),
            )
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
            if read(uart0::LSR) & uart0::THRE == 0 {
                return None;
            }
            let end = (self.offset + uart0::FIFO_BYTES).min(r.bytes.len());
            for &byte in &r.bytes[self.offset..end] {
                write(uart0::DATA, u32::from(byte));
            }
            self.offset = end;
        }
        if self.offset != r.bytes.len() {
            return None;
        }
        if matches!(r.kind, Kind::Result(_) | Kind::Reboot) && read(uart0::LSR) & uart0::TEMT == 0 {
            return None;
        }
        self.offset = 0;
        Some(self.remove(0))
    }
}
