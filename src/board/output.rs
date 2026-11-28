//! Bounded binary UART records. Only an unstarted record may be displaced.
use super::{lpc2146::uart0, write};
use tekmar_482_firmware::diagnostics::errors::{ErrorBook, LineError};
use tekmar_482_firmware::tn4::{MAX_BYTES, PORT_COUNT};
use tekmar_482_firmware::tpck;
use tekmar_482_firmware::trpc::MAX_REPLY_BYTES;
const REPORT_WATERMARK: usize = 2;
const RX_WATERMARK: usize = 6;
const COMMAND_WATERMARK: usize = 8;
// Two simultaneous five-line bursts can arrive before the UART drains a record.
const TRACE_SLOTS: usize = 2 * PORT_COUNT;
const RECORD_SLOTS: usize = COMMAND_WATERMARK + PORT_COUNT + TRACE_SLOTS;
type SlotMask = u32;
const _: () = assert!(RECORD_SLOTS <= SlotMask::BITS as usize);
const INVALID_RECORD_BYTES: usize = tpck::raw::INVALID_PREFIX + MAX_BYTES + size_of::<u16>();
const RECORD_PAYLOAD_BYTES: usize = if INVALID_RECORD_BYTES > MAX_REPLY_BYTES {
    INVALID_RECORD_BYTES
} else {
    MAX_REPLY_BYTES
};
const _: () = assert!(RECORD_PAYLOAD_BYTES <= u8::MAX as usize);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Rx(u8),
    Invalid(u8),
    Result(u8),
    Overlap(u8),
    Control,
    Trace,
    Diagnostic,
    Fence,
    Mode,
    Reboot,
}

struct Record {
    payload: [u8; RECORD_PAYLOAD_BYTES],
    len: u8,
    ty: u8,
    kind: Kind,
}

pub(super) struct Output {
    records: [Record; RECORD_SLOTS],
    order: [u8; RECORD_SLOTS],
    used: SlotMask,
    len: usize,
    traces: usize,
    gap: tpck::trace::PendingGap,
    encoder: tpck::Encoder,
    diagnostic: bool,
}

impl Output {
    pub(super) fn take() -> &'static mut Self {
        // Auxiliary SRAM is CPU-accessible; keep the stack and link drivers in local SRAM.
        #[unsafe(link_section = ".bss.uart_output")]
        static QUEUE: static_cell::ConstStaticCell<Output> =
            static_cell::ConstStaticCell::new(Output {
                records: [const {
                    Record {
                        payload: [0; RECORD_PAYLOAD_BYTES],
                        len: 0,
                        ty: 0,
                        kind: Kind::Rx(0),
                    }
                }; RECORD_SLOTS],
                order: [0; RECORD_SLOTS],
                used: 0,
                len: 0,
                traces: 0,
                gap: tpck::trace::PendingGap::EMPTY,
                encoder: tpck::Encoder::EMPTY,
                diagnostic: false,
            });
        QUEUE.take()
    }
    pub(super) fn can_report(&self) -> bool {
        self.len - self.traces < REPORT_WATERMARK
    }
    pub(super) fn can_receive(&self) -> bool {
        self.len - self.traces < COMMAND_WATERMARK
    }
    pub(super) fn can_dispatch(&self) -> bool {
        // A UART boundary may also have to enqueue an older trace loss marker.
        self.len - self.traces + usize::from(self.gap.pending()) < COMMAND_WATERMARK
    }
    pub(super) fn boundary(&mut self, kind: Kind, reply: &[u8]) {
        self.flush_trace_gap(true);
        self.enqueue(kind, tpck::TRPC, reply);
    }
    pub(super) fn set_header(&mut self, header: tpck::Header) {
        self.encoder.set_header(header);
    }
    pub(super) fn can_diagnose(&self) -> bool {
        !self.diagnostic && self.can_report()
    }
    pub(super) fn trace(
        &mut self,
        event: u8,
        p: usize,
        ms: u32,
        detail: &[u8],
        body: &[u8],
        errors: &mut ErrorBook,
    ) {
        self.flush_trace_gap(false);
        if self.traces == TRACE_SLOTS {
            errors.line(p, LineError::TraceDrop, ms);
            self.gap.lost(p);
            return;
        }
        let mut prefix = [0; tpck::raw::RX_PREFIX];
        prefix[0] = event;
        prefix[1] = p as u8;
        prefix[2..].copy_from_slice(&ms.to_le_bytes());
        self.enqueue_parts(Kind::Trace, tpck::NORMAL_TRACE, &[&prefix, detail, body]);
    }
    pub(super) fn flush_trace_gap(&mut self, boundary: bool) {
        if (self.traces < TRACE_SLOTS || boundary)
            && let Some(data) = self.gap.take()
        {
            let kind = if self.traces < TRACE_SLOTS {
                Kind::Trace
            } else {
                Kind::Fence
            };
            self.enqueue(kind, tpck::NORMAL_TRACE, &data);
        }
    }
    fn lost(kind: Kind, ms: u32, errors: &mut ErrorBook) {
        let (p, code) = match kind {
            Kind::Rx(p) => (p, LineError::RawValidDrop),
            Kind::Invalid(p) => (p, LineError::RawInvalidDrop),
            _ => unreachable!(),
        };
        errors.line(usize::from(p), code, ms);
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
        } else if kind == Kind::Diagnostic {
            self.diagnostic = false;
        }
        kind
    }
    pub(super) fn capture(&mut self, kind: Kind, parts: &[&[u8]], ms: u32, errors: &mut ErrorBook) {
        let first = usize::from(self.encoder.started());
        if self.len - self.traces >= RX_WATERMARK {
            if matches!(kind, Kind::Rx(_))
                && let Some(i) = self.order[first..self.len]
                    .iter()
                    .position(|&s| matches!(self.records[usize::from(s)].kind, Kind::Invalid(_)))
            {
                let old = self.remove(first + i);
                Self::lost(old, ms, errors);
            } else {
                Self::lost(kind, ms, errors);
                return;
            }
        }
        self.enqueue_parts(kind, tpck::TN4, parts);
    }
    pub(super) fn enqueue(&mut self, kind: Kind, ty: u8, data: &[u8]) {
        self.enqueue_parts(kind, ty, &[data]);
    }
    fn enqueue_parts(&mut self, kind: Kind, ty: u8, parts: &[&[u8]]) {
        let first = usize::from(self.encoder.started());
        let pos = if let Kind::Overlap(port) = kind {
            self.order[first..self.len]
                .iter()
                .position(|&s| self.records[usize::from(s)].kind == Kind::Result(port))
                .map(|n| first + n)
        } else if kind == Kind::Control && (self.traces != 0 || self.diagnostic) {
            // Control may overtake pending diagnostics, but never a partial
            // frame, a trace configuration fence, or an older control record.
            Some(
                self.order[first..self.len]
                    .iter()
                    .rposition(|&s| {
                        !matches!(
                            self.records[usize::from(s)].kind,
                            Kind::Trace | Kind::Diagnostic
                        )
                    })
                    .map_or(first, |i| first + i + 1),
            )
        } else {
            None
        };
        let slot = (!self.used).trailing_zeros() as usize;
        self.used |= 1 << slot;
        let r = &mut self.records[slot];
        r.kind = kind;
        r.ty = ty;
        let mut tail = &mut r.payload[..];
        for data in parts {
            let (part, rest) = tail.split_at_mut(data.len());
            part.copy_from_slice(data);
            tail = rest;
        }
        r.len = (RECORD_PAYLOAD_BYTES - tail.len()) as u8;
        self.order[self.len] = slot as u8;
        for i in (pos.unwrap_or(self.len)..self.len).rev() {
            self.order.swap(i, i + 1);
        }
        self.len += 1;
        if kind == Kind::Trace {
            self.traces += 1;
        } else if kind == Kind::Diagnostic {
            self.diagnostic = true;
        }
    }
    pub(super) fn pump(
        &mut self,
        uart: &mut super::UartRx,
        errors: &mut ErrorBook,
        ms: u32,
    ) -> Option<Kind> {
        if self.len == 0 {
            return None;
        }
        let r = &self.records[usize::from(self.order[0])];
        if !self.encoder.finished() {
            if uart.status(errors, ms) & uart0::THRE == 0 {
                return None;
            }
            self.encoder
                .write(
                    r.ty,
                    &r.payload[..usize::from(r.len)],
                    uart0::FIFO_BYTES,
                    |byte| {
                        write(uart0::DATA, u32::from(byte));
                    },
                )
                .unwrap();
        }
        if !self.encoder.finished() {
            return None;
        }
        if matches!(r.kind, Kind::Result(_) | Kind::Mode | Kind::Reboot)
            && uart.status(errors, ms) & uart0::TEMT == 0
        {
            return None;
        }
        self.encoder.reset();
        Some(self.remove(0))
    }
}
