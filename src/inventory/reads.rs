//! Targeted Normal-mode cache reads. These never own or delay a host transaction.
use super::{BLOCK_REQUEST_FRAME_BYTES, Device, Inventory, NODES, block, block_request};
use crate::uptime::Sample;

// Give the peer time to return an unanswered block on this line. A matching
// report releases it immediately, even within the same millisecond.
const BACKGROUND_REPLY_WINDOW_MS: u32 = 1_000;

// One bit per supported read, not per wire ID. Resolve dependencies first.
const READS: &[u8] = &[
    block::SENSOR_SETUP,
    block::SETPOINT_SETUP,
    block::DEVICE_INFO,
    block::HEATING_SETUP,
    block::HEAT_SETPOINTS,
    block::COOL_SETPOINTS,
    block::SLAB_SETPOINTS,
    block::FAN_PERCENT,
    block::HUMIDITY_SETUP,
    block::HUMIDIFY_TARGETS,
    block::DEHUMIDIFY_TARGETS,
];
const _: () = assert!(READS.len() <= u16::BITS as usize && NODES <= u32::BITS as usize);

fn mask(id: u8) -> u16 {
    READS
        .iter()
        .position(|&code| code == id)
        .map_or(0, |i| 1 << i)
}

fn next_bit(bits: u32, cursor: u32) -> u32 {
    let rest = bits & (u32::MAX << cursor);
    (if rest != 0 { rest } else { bits }).trailing_zeros()
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    node: u8,
    block: u8,
}

#[derive(Clone, Copy)]
pub struct BlockProbe {
    key: Key,
    port: u8,
    subnet: u8,
    incarnation: u16,
}

impl BlockProbe {
    pub fn body(self) -> [u8; BLOCK_REQUEST_FRAME_BYTES] {
        block_request(self.key.block, self.subnet, usize::from(self.key.node))
    }
}

#[derive(Default)]
pub(super) struct Reads {
    pending: [u16; NODES],
    next_block: [u8; NODES],
    nodes: u32,
    cursor: u32,
    waiting: Option<(Key, u32)>,
}

impl Reads {
    pub(super) const EMPTY: Self = Self {
        pending: [0; NODES],
        next_block: [0; NODES],
        nodes: 0,
        cursor: 0,
        waiting: None,
    };
    pub(super) fn forget(&mut self, node: usize) {
        self.pending[node] = 0;
        self.next_block[node] = 0;
        self.nodes &= !(1 << node);
        if self
            .waiting
            .is_some_and(|(k, _)| usize::from(k.node) == node)
        {
            self.waiting = None;
        }
    }
    pub(super) fn reported(&mut self, node: usize, block: u8) {
        let key = Key {
            node: node as u8,
            block,
        };
        if self.waiting.is_some_and(|(k, _)| k == key) {
            self.waiting = None;
        }
    }
    pub(super) fn discard(&mut self, node: usize, id: u8) {
        self.pending[node] &= !mask(id);
        if self.pending[node] == 0 {
            self.nodes &= !(1 << node);
        }
    }
    pub(super) fn queue(&mut self, node: usize, block: u8) {
        let bit = mask(block);
        if bit != 0 {
            self.pending[node] |= bit;
            self.nodes |= 1 << node;
        }
    }
    fn take(&mut self, now: u32) -> Option<Key> {
        if let Some((_, since)) = self.waiting {
            if now.wrapping_sub(since) < BACKGROUND_REPLY_WINDOW_MS {
                return None;
            }
            self.waiting = None;
        }
        if self.nodes == 0 {
            return None;
        }
        let node = next_bit(self.nodes, self.cursor) as usize;
        self.cursor = ((node + 1) % NODES) as u32;
        let i = next_bit(
            u32::from(self.pending[node]),
            u32::from(self.next_block[node]),
        ) as usize;
        self.next_block[node] = ((i + 1) % READS.len()) as u8;
        let id = READS[i];
        self.discard(node, id);
        Some(Key {
            node: node as u8,
            block: id,
        })
    }
}

impl Inventory {
    pub fn has_reads(&self, port: usize) -> bool {
        let reads = &self.ports[port].reads;
        reads.nodes != 0 || reads.waiting.is_some()
    }
    pub fn next_probe(&mut self, p: usize, time: Sample) -> Option<BlockProbe> {
        let port = &mut self.ports[p];
        if port.job.active() || self.broadcasts.pending(p) {
            return None;
        }
        while let Some(key) = port.reads.take(time.millis) {
            let row = &port.devices[usize::from(key.node)];
            if row.present(time.seconds) && row.needs_probe(key.block) {
                port.reads.waiting = Some((key, time.millis));
                return Some(BlockProbe {
                    key,
                    port: p as u8,
                    subnet: row.subnet,
                    incarnation: row.incarnation,
                });
            }
        }
        None
    }

    /// A queued, not-yet-driven read must not migrate to a replacement device.
    pub fn probe_current(&self, probe: BlockProbe, now: u32) -> bool {
        let port = &self.ports[usize::from(probe.port)];
        let row = &port.devices[usize::from(probe.key.node)];
        row.present(now)
            && row.incarnation == probe.incarnation
            && row.needs_probe(probe.key.block)
            && port.reads.waiting.is_some_and(|(k, _)| k == probe.key)
    }
    pub fn probe_sent(&mut self, probe: BlockProbe, millis: u32) {
        if let Some((k, since)) = &mut self.ports[usize::from(probe.port)].reads.waiting
            && *k == probe.key
        {
            *since = millis;
        }
    }
    pub fn defer_probe(&mut self, probe: BlockProbe, now: u32) {
        let needed = self.probe_current(probe, now);
        let reads = &mut self.ports[usize::from(probe.port)].reads;
        let node = usize::from(probe.key.node);
        reads.reported(node, probe.key.block);
        if needed {
            reads.queue(node, probe.key.block);
        }
    }
}

impl Device {
    fn needs_probe(&self, id: u8) -> bool {
        match id {
            block::DEVICE_INFO => !self.identity.complete(),
            block::SETPOINT_SETUP => self.live.is_setpoint_device(),
            block::SENSOR_SETUP => true,
            block::HEATING_SETUP => self.live.configured(super::Setting::Mode),
            block::HUMIDITY_SETUP => self.live.has_rh(),
            _ => self.live.missing_target(id),
        }
    }
}
