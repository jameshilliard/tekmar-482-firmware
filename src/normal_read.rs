//! Explicit, single-peer Normal reads. A link ACK is never an application reply.
use crate::tn4::{
    self, HEADER_BYTES, MAX_BYTES, NODE_MASK, message,
    tx::{Outcome, Request},
};
use crate::trpc::{Reply, method};

pub const WINDOW_MS: u32 = 10_000;
const RESULT_PREFIX: usize = 2 + size_of::<u32>(); // port, status, observation time
pub const MAX_RESULT: usize = RESULT_PREFIX + MAX_BYTES;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Report = 0,
    Expired = 1,
    PinUnavailable = 2,
    LinkFault = 3,
}

#[cfg(test)]
mod tests;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Queued,
    Listening,
    Done(Status),
}

#[derive(Clone, Copy)]
pub struct Query {
    node: u8,
    subnet: u8,
    block: [u8; 2],
    selector_mask: u8,
    selector: u8,
}

impl Query {
    pub fn parse(body: &[u8]) -> Option<Self> {
        let &[
            subnet,
            node @ 1..=NODE_MASK,
            lo,
            hi,
            selector_mask,
            selector,
        ] = body
        else {
            return None;
        };
        (selector & !selector_mask == 0).then_some(Self {
            node,
            subnet,
            block: [lo, hi],
            selector_mask,
            selector,
        })
    }

    fn frame(self, port: u8) -> Option<Request> {
        let mut data = [0; 11]; // block, subnet, node bitmap, little-endian u32 argument
        data[..2].copy_from_slice(&self.block);
        data[2] = self.subnet;
        data[3..7].copy_from_slice(&(1u32 << (self.node - 1)).to_le_bytes());
        data[7] = self.selector;
        Request::gateway(port, message::BLOCK_REQUEST, &data)
    }

    fn matches(self, body: &[u8]) -> bool {
        let [a, b, c, d, lo, hi, support, data @ ..] = body else {
            return false;
        };
        let header = u32::from_be_bytes([*a, *b, *c, *d]);
        let count = support.count_ones() as usize;
        let span = (u8::BITS - support.leading_zeros()) as usize;
        (header >> tn4::MESSAGE_SHIFT) & tn4::MESSAGE_MASK == message::BLOCK_REPORT
            && (header >> tn4::SUBNET_SHIFT) as u8 == self.subnet
            && (header >> tn4::NODE_SHIFT) as u8 & NODE_MASK == self.node
            && (header as usize & tn4::PAYLOAD_MASK as usize) + HEADER_BYTES == body.len()
            && [*lo, *hi] == self.block
            && (data.len() == count || data.len() >= span)
            && (self.selector_mask == 0
                || support & 1 != 0 && data[0] & self.selector_mask == self.selector)
    }
}

pub struct BlockRead {
    phase: Phase,
    query: Query,
    stamp: u32,
    body: [u8; MAX_BYTES],
    len: u8,
}

impl BlockRead {
    pub const EMPTY: Self = Self {
        phase: Phase::Idle,
        query: Query {
            node: 0,
            subnet: 0,
            block: [0; 2],
            selector_mask: 0,
            selector: 0,
        },
        stamp: 0,
        body: [0; MAX_BYTES],
        len: 0,
    };
    pub fn active(&self) -> bool {
        self.phase != Phase::Idle
    }
    pub fn queued(&self) -> bool {
        self.phase == Phase::Queued
    }
    pub fn admit(&mut self, query: Query, now: u32) -> bool {
        if self.active() {
            return false;
        }
        self.query = query;
        self.stamp = now;
        self.len = 0;
        self.phase = Phase::Queued;
        true
    }
    pub fn frame(&self, port: u8) -> Option<Request> {
        self.queued().then(|| self.query.frame(port)).flatten()
    }
    pub fn sent(&mut self, outcome: Outcome) {
        if self.queued() {
            self.phase = match outcome {
                Outcome::ArbitrationLost => Phase::Queued,
                Outcome::Ack | Outcome::NoAck => Phase::Listening,
                _ => Phase::Done(Status::LinkFault),
            }
        }
    }
    pub fn unavailable(&mut self) {
        if self.queued() {
            self.phase = Phase::Done(Status::PinUnavailable);
        }
    }
    pub fn expire(&mut self, now: u32) {
        if matches!(self.phase, Phase::Queued | Phase::Listening)
            && now.wrapping_sub(self.stamp) >= WINDOW_MS
        {
            self.phase = Phase::Done(Status::Expired);
        }
    }
    pub fn observe(&mut self, body: &[u8], now: u32) {
        if self.phase == Phase::Listening && self.query.matches(body) {
            self.stamp = now;
            self.body[..body.len()].copy_from_slice(body);
            self.len = body.len() as u8;
            self.phase = Phase::Done(Status::Report);
        }
    }
    /// Retire only when the caller can put the terminal event in the control FIFO.
    pub fn take_result(&mut self, port: u8) -> Option<Reply> {
        let Phase::Done(status) = self.phase else {
            return None;
        };
        self.phase = Phase::Idle;
        let mut data = [0; MAX_RESULT];
        data[0] = port;
        data[1] = status as u8;
        let len = if status == Status::Report {
            data[2..RESULT_PREFIX].copy_from_slice(&self.stamp.to_le_bytes());
            let len = usize::from(self.len);
            data[RESULT_PREFIX..RESULT_PREFIX + len].copy_from_slice(&self.body[..len]);
            RESULT_PREFIX + len
        } else {
            2
        };
        Some(Reply::report(method::NORMAL_BLOCK_READ, &data[..len]))
    }
}
