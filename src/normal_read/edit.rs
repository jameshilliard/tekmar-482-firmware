//! Guard a single addressed block update with a fresh, exact peer baseline.
use super::{Query, TX_NOT_SENT, TX_PIN_UNAVAILABLE, WINDOW_MS, outcome_byte};
use crate::tn4::{
    self, HEADER_BYTES, NODE_MASK, message,
    tx::{Outcome, Request},
};
use crate::trpc::{Reply, method};

const MAX_FIELDS: usize = u8::BITS as usize; // one support bit per positional byte
const TARGET_PREFIX: usize = 4; // subnet, node, block u16
const QUERY_BYTES: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum End {
    Attempted = 0,
    Changed = 1,
    Expired = 2,
    BaselineFailed = 3,
    UpdateUnavailable = 4,
    TargetMoved = 5,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Read,
    Listening,
    Write,
    Done(End),
    Idle,
}

// Positional sizes and selector masks of the 483 block writers and tank reset. A reader
// existing for a block is not evidence that a writer exists for it.
pub(super) fn layout(block: u16) -> Option<(usize, u8)> {
    Some(match block {
        0x03 => (3, 0),
        0x05 | 0x06 | 0x09 | 0xa4 | 0xa5 => (5, 0),
        0x07 | 0x0f | 0x25 => (6, 0),
        0x0e | 0x24 | 0x26 | 0x27 | 0x28 | 0x2a | 0x2c | 0x2e | 0x30 => (8, 0),
        0x10..=0x13 => (7, 0),
        0x1c..=0x21 | 0x4c | 0x71 | 0x72 | 0x78 | 0x99..=0xa2 => (2, 0),
        0x29 | 0x2b | 0x2d | 0x2f | 0x31 => (4, 0),
        0x38..=0x3b => (8, 7),
        0x43 | 0x44 | 0x5a | 0x98 => (3, 0xff),
        _ => return None,
    })
}

pub struct BlockEdit {
    query: Query,
    node: u8,
    support: u8,
    before: heapless::Vec<u8, MAX_FIELDS>,
    proposed: heapless::Vec<u8, MAX_FIELDS>,
    phase: Phase,
    start: u32,
    baseline_tx: u8,
    update_tx: u8,
}

impl BlockEdit {
    pub(super) fn parse(key: &[u8], start: u32) -> Option<Self> {
        let (q, rest) = key.split_at_checked(QUERY_BYTES)?;
        let &[subnet, node @ 1..=NODE_MASK, lo, hi, mask, selector] = q else {
            return None;
        };
        let query = Query::new(subnet, 1 << (node - 1), [lo, hi], mask, selector)?;
        let &[support, len, ref fields @ ..] = rest else {
            return None;
        };
        let block = u16::from_le_bytes(query.block);
        let (mut size, mask) = layout(block)?;
        let (before, proposed) = fields.split_at_checked(usize::from(len))?;
        if block == 0x2e && before.len() == 6 {
            size = 6;
        }
        let pad = block == 0x0f && before.len() == 5;
        if before.len() + usize::from(pad) != size
            || proposed.len() != size
            || support == 0
            || support.ilog2() as usize >= before.len()
            || query.selector_mask != mask
            || (mask != 0
                && (support & 1 == 0
                    || before[0] & mask != query.selector
                    || proposed[0] & mask != query.selector))
            || (block == 0x0f && (proposed[5] != 0 || before.get(5).copied().unwrap_or(0) != 0))
            || (block == 0x5a && proposed[1..] != [0, 0])
            || before
                .iter()
                .zip(proposed)
                .enumerate()
                .any(|(i, (a, b))| a != b && support & (1 << i) == 0)
            || before == &proposed[..before.len()]
        {
            return None;
        }
        Some(Self {
            query,
            node,
            support,
            before: heapless::Vec::from_slice(before).ok()?,
            proposed: heapless::Vec::from_slice(proposed).ok()?,
            phase: Phase::Read,
            start,
            baseline_tx: TX_NOT_SENT,
            update_tx: TX_NOT_SENT,
        })
    }
    pub(super) fn writing(&self) -> bool {
        self.phase == Phase::Write
    }
    pub(super) fn heating_target(&self) -> Option<(u8, u8)> {
        (self.query.block == [crate::tn4::block::HEATING_SETUP, 0])
            .then_some((self.query.subnet, self.node))
    }
    pub(super) fn queued(&self) -> bool {
        matches!(self.phase, Phase::Read | Phase::Write)
    }
    pub(super) fn frame(&self, port: u8) -> Option<Request> {
        match self.phase {
            Phase::Read => self.query.frame(port),
            Phase::Write => {
                let mut payload = [0; TARGET_PREFIX + MAX_FIELDS];
                payload[0] = self.query.subnet;
                payload[1] = self.node;
                payload[2..TARGET_PREFIX].copy_from_slice(&self.query.block);
                let end = TARGET_PREFIX + self.proposed.len();
                payload[TARGET_PREFIX..end].copy_from_slice(&self.proposed);
                Request::gateway(port, message::BLOCK_UPDATE, &payload[..end])
            }
            _ => None,
        }
    }
    pub(super) fn sent(&mut self, outcome: Outcome) {
        match self.phase {
            Phase::Read if outcome != Outcome::ArbitrationLost => {
                self.baseline_tx = outcome_byte(outcome);
                self.phase = if matches!(outcome, Outcome::Ack | Outcome::NoAck) {
                    Phase::Listening
                } else {
                    Phase::Done(End::BaselineFailed)
                };
            }
            Phase::Write => {
                self.update_tx = outcome_byte(outcome);
                self.phase = Phase::Done(End::Attempted);
            }
            _ => {}
        }
    }
    pub(super) fn observe(&mut self, body: &[u8]) {
        if matches!(self.phase, Phase::Read | Phase::Listening | Phase::Write)
            && self.address_changed(body)
        {
            self.phase = Phase::Done(End::TargetMoved);
            return;
        }
        if matches!(self.phase, Phase::Listening | Phase::Write)
            && self.query.matching_bit(body).is_some()
        {
            let report = &body[HEADER_BYTES + 2..]; // after block ID
            self.phase = if report[0] == self.support && report[1..] == self.before[..] {
                Phase::Write
            } else {
                Phase::Done(End::Changed)
            };
        }
    }
    fn address_changed(&self, body: &[u8]) -> bool {
        let [a, b, c, d, old_subnet, old_node, ..] = body else {
            return false;
        };
        let header = u32::from_be_bytes([*a, *b, *c, *d]);
        let subnet = (header >> tn4::SUBNET_SHIFT) as u8;
        let node = (header >> tn4::NODE_SHIFT) as u8 & NODE_MASK;
        (header >> tn4::MESSAGE_SHIFT) & tn4::MESSAGE_MASK == message::TAKING_ADDRESS
            && node != 0
            && ((self.query.subnet, self.node) == (*old_subnet, *old_node & NODE_MASK)
                || (self.query.subnet, self.node) == (subnet, node))
    }
    pub(super) fn unavailable(&mut self) {
        match self.phase {
            Phase::Read => {
                self.baseline_tx = TX_PIN_UNAVAILABLE;
                self.phase = Phase::Done(End::BaselineFailed);
            }
            Phase::Write => {
                self.update_tx = TX_PIN_UNAVAILABLE;
                self.phase = Phase::Done(End::UpdateUnavailable);
            }
            _ => {}
        }
    }
    pub(super) fn expire(&mut self, now: u32) {
        if !matches!(self.phase, Phase::Done(_) | Phase::Idle)
            && now.wrapping_sub(self.start) >= WINDOW_MS
        {
            self.phase = Phase::Done(End::Expired);
        }
    }
    pub(super) fn take_result(&mut self, port: u8) -> Option<Reply> {
        let Phase::Done(end) = self.phase else {
            return None;
        };
        self.phase = Phase::Idle;
        Some(Reply::report(
            method::NORMAL_BLOCK_EDIT,
            &[port, end as u8, self.baseline_tx, self.update_tx],
        ))
    }
}
