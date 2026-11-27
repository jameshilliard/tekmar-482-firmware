//! Explicit Normal operations sharing a physical line. A link ACK is never an application reply.
use crate::tn4::{
    self, HEADER_BYTES, MAX_BYTES, NODE_MASK, message,
    tx::{Outcome, Request},
};
use crate::tpck::raw;
use crate::trpc::{BridgeError, Reply, method};
mod edit;
mod send;
mod status;
use edit::BlockEdit;
use send::Send;
use status::StatusRead;

pub const WINDOW_MS: u32 = 10_000;
const RESULT_PREFIX: usize = 2 + size_of::<u32>(); // port, status, observation time
const BLOCK_PREFIX: usize = RESULT_PREFIX + 1; // also includes the observed TX outcome
pub const MAX_RESULT: usize = BLOCK_PREFIX + MAX_BYTES;
const TX_PIN_UNAVAILABLE: u8 = 0xfe;
const TX_NOT_SENT: u8 = 0xff;
const ALL_NODES: u32 = u32::MAX >> 1;

pub fn outcome_byte(result: Outcome) -> u8 {
    match result {
        Outcome::Ack => raw::status::ACKNOWLEDGED,
        Outcome::NoAck => raw::attempted::NO_ACK,
        Outcome::ArbitrationLost => raw::attempted::ARBITRATION_LOST,
        Outcome::EchoMismatch => raw::attempted::ECHO_MISMATCH,
        Outcome::BadEnd => raw::attempted::BAD_END,
        Outcome::Late => raw::attempted::MISSED_TIMING,
    }
}

#[repr(u8)]
pub enum NormalOperation {
    Block(BlockRead),
    Status(StatusRead),
    Edit(BlockEdit),
    Send(Send),
}

impl NormalOperation {
    pub const EMPTY: Self = Self::Block(BlockRead::EMPTY);

    pub fn parse(method: u32, key: &[u8], now: u32) -> Result<Self, BridgeError> {
        match method {
            method::NORMAL_BLOCK_READ => {
                let q = Query::parse(key).ok_or(BridgeError::InvalidRequest)?;
                let mut block = BlockRead::EMPTY;
                block.admit(q, now);
                Ok(Self::Block(block))
            }
            method::NORMAL_STATUS_READ => StatusRead::parse(key, now)
                .map(Self::Status)
                .ok_or(BridgeError::InvalidRequest),
            method::NORMAL_BLOCK_EDIT => BlockEdit::parse(key, now)
                .map(Self::Edit)
                .ok_or(BridgeError::InvalidRequest),
            method::NORMAL_ANNOUNCE => Send::announcement(key, now)
                .map(Self::Send)
                .ok_or(BridgeError::InvalidRequest),
            method::NORMAL_DISCOVER => Send::discovery(key, now)
                .map(Self::Send)
                .ok_or(BridgeError::InvalidRequest),
            method::NORMAL_STAGE_COMMAND => Send::stage(key, now)
                .map(Self::Send)
                .ok_or(BridgeError::InvalidRequest),
            _ => Err(BridgeError::InvalidRequest),
        }
    }
    pub fn single_attempt(&self) -> bool {
        matches!(self, Self::Send(_)) || matches!(self, Self::Edit(r) if r.writing())
    }
    pub fn heating_edit_target(&self) -> Option<(u8, u8)> {
        match self {
            Self::Edit(edit) => edit.heating_target(),
            _ => None,
        }
    }
    pub fn trace_origin(&self) -> u8 {
        use crate::tpck::trace;
        match self {
            Self::Send(r) => r.trace_origin(),
            Self::Edit(r) if r.writing() => trace::SETTING,
            _ => trace::PROBE,
        }
    }
    pub fn queued(&self) -> bool {
        match self {
            Self::Block(r) => r.queued(),
            Self::Status(r) => r.queued(),
            Self::Edit(r) => r.queued(),
            Self::Send(r) => r.queued(),
        }
    }
    pub fn frame(&self, port: u8) -> Option<Request> {
        match self {
            Self::Block(r) => r.frame(port),
            Self::Status(r) => r.frame(port),
            Self::Edit(r) => r.frame(port),
            Self::Send(r) => r.frame(port),
        }
    }
    pub fn sent(&mut self, outcome: Outcome, now: u32) {
        match self {
            Self::Block(r) => r.sent(outcome, now),
            Self::Status(r) => r.sent(outcome),
            Self::Edit(r) => r.sent(outcome),
            Self::Send(r) => r.sent(outcome),
        }
    }
    pub fn unavailable(&mut self) {
        match self {
            Self::Block(r) => r.unavailable(),
            Self::Status(r) => r.unavailable(),
            Self::Edit(r) => r.unavailable(),
            Self::Send(r) => r.unavailable(),
        }
    }
    pub fn expire(&mut self, now: u32) {
        match self {
            Self::Block(r) => r.expire(now),
            Self::Status(r) => r.expire(now),
            Self::Edit(r) => r.expire(now),
            Self::Send(r) => r.expire(now),
        }
    }
    pub fn observe(
        &mut self,
        port: u8,
        body: &[u8],
        now: u32,
        publish: impl FnOnce(&Reply) -> bool,
    ) -> bool {
        match self {
            Self::Block(r) => r.observe(port, body, now, publish),
            Self::Status(r) => r.observe(port, body, now, publish),
            Self::Send(_) => true,
            Self::Edit(r) => {
                r.observe(body);
                true
            }
        }
    }
    pub fn take_result(&mut self, port: u8) -> Option<Reply> {
        match self {
            Self::Block(r) => r.take_result(port),
            Self::Status(r) => r.take_result(port),
            Self::Edit(r) => r.take_result(port),
            Self::Send(r) => r.take_result(port),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Report = 0,
    Expired = 1,
    PinUnavailable = 2,
    LinkFault = 3,
    More = 4,
    ControlFull = 5,
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
    nodes: u32,
    subnet: u8,
    block: [u8; 2],
    selector_mask: u8,
    selector: u8,
}

impl Query {
    pub fn parse(body: &[u8]) -> Option<Self> {
        let &[subnet, a, b, c, d, lo, hi, selector_mask, selector] = body else {
            return None;
        };
        Self::new(
            subnet,
            u32::from_le_bytes([a, b, c, d]),
            [lo, hi],
            selector_mask,
            selector,
        )
    }

    fn new(
        subnet: u8,
        nodes: u32,
        block: [u8; 2],
        selector_mask: u8,
        selector: u8,
    ) -> Option<Self> {
        (nodes != 0 && nodes <= ALL_NODES && selector & !selector_mask == 0).then_some(Self {
            nodes,
            subnet,
            block,
            selector_mask,
            selector,
        })
    }

    fn frame(self, port: u8) -> Option<Request> {
        let mut data = [0; 11]; // block, subnet, node bitmap, little-endian u32 argument
        data[..2].copy_from_slice(&self.block);
        data[2] = self.subnet;
        data[3..7].copy_from_slice(&self.nodes.to_le_bytes());
        data[7] = self.selector;
        Request::gateway(port, message::BLOCK_REQUEST, &data)
    }

    fn matching_bit(self, body: &[u8]) -> Option<u32> {
        let [a, b, c, d, lo, hi, support, data @ ..] = body else {
            return None;
        };
        let header = u32::from_be_bytes([*a, *b, *c, *d]);
        let node = (header >> tn4::NODE_SHIFT) as u8 & NODE_MASK;
        let bit = 1u32 << node.checked_sub(1)?;
        let count = support.count_ones() as usize;
        let span = (u8::BITS - support.leading_zeros()) as usize;
        ((header >> tn4::MESSAGE_SHIFT) & tn4::MESSAGE_MASK == message::BLOCK_REPORT
            && (header >> tn4::SUBNET_SHIFT) as u8 == self.subnet
            && self.nodes & bit != 0
            && (header as usize & tn4::PAYLOAD_MASK as usize) + HEADER_BYTES == body.len()
            && [*lo, *hi] == self.block
            && (data.len() == count || data.len() >= span)
            && (self.selector_mask == 0
                || support & 1 != 0 && data[0] & self.selector_mask == self.selector))
            .then_some(bit)
    }
}

pub struct BlockRead {
    phase: Phase,
    query: Query,
    stamp: u32,
    body: [u8; MAX_BYTES],
    len: u8,
    tx: u8,
}

impl BlockRead {
    pub const EMPTY: Self = Self {
        phase: Phase::Idle,
        query: Query {
            nodes: 0,
            subnet: 0,
            block: [0; 2],
            selector_mask: 0,
            selector: 0,
        },
        stamp: 0,
        body: [0; MAX_BYTES],
        len: 0,
        tx: 0, // Idle storage is zero-initialized; admission sets TX_NOT_SENT.
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
        self.tx = TX_NOT_SENT;
        self.phase = Phase::Queued;
        true
    }
    pub fn frame(&self, port: u8) -> Option<Request> {
        self.queued().then(|| self.query.frame(port)).flatten()
    }
    pub fn sent(&mut self, outcome: Outcome, now: u32) {
        if self.queued() {
            if outcome != Outcome::ArbitrationLost {
                self.tx = outcome_byte(outcome);
            }
            self.phase = match outcome {
                Outcome::ArbitrationLost => Phase::Queued,
                Outcome::Ack | Outcome::NoAck => {
                    self.stamp = now;
                    Phase::Listening
                }
                _ => Phase::Done(Status::LinkFault),
            }
        }
    }
    pub fn unavailable(&mut self) {
        if self.queued() {
            self.tx = TX_PIN_UNAVAILABLE;
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
    pub fn observe(
        &mut self,
        port: u8,
        body: &[u8],
        now: u32,
        publish: impl FnOnce(&Reply) -> bool,
    ) -> bool {
        if self.phase != Phase::Listening {
            return true;
        }
        let Some(bit) = self.query.matching_bit(body) else {
            return true;
        };
        self.query.nodes &= !bit;
        self.stamp = now;
        if self.query.nodes == 0 {
            self.body[..body.len()].copy_from_slice(body);
            self.len = body.len() as u8;
            self.phase = Phase::Done(Status::Report);
        } else if !publish(&self.result(port, Status::More, body)) {
            self.phase = Phase::Done(Status::ControlFull);
            return false;
        }
        true
    }
    /// Retire only when the caller can put the terminal event in the control FIFO.
    pub fn take_result(&mut self, port: u8) -> Option<Reply> {
        let Phase::Done(status) = self.phase else {
            return None;
        };
        self.phase = Phase::Idle;
        Some(self.result(port, status, &self.body[..usize::from(self.len)]))
    }
    fn result(&self, port: u8, status: Status, body: &[u8]) -> Reply {
        let mut data = [0; MAX_RESULT];
        data[0] = port;
        data[1] = status as u8;
        data[2] = self.tx;
        let len = if matches!(status, Status::Report | Status::More) {
            data[3..BLOCK_PREFIX].copy_from_slice(&self.stamp.to_le_bytes());
            data[BLOCK_PREFIX..BLOCK_PREFIX + body.len()].copy_from_slice(body);
            BLOCK_PREFIX + body.len()
        } else {
            3
        };
        Reply::report(method::NORMAL_BLOCK_READ, &data[..len])
    }
}
