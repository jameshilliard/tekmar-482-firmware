//! Normal-mode inventory. Bus addresses, not product numbers, identify entries.
use crate::trpc::{Call, Reply};

const NODES: usize = 24;
const PORTS: usize = 5;
const SLOTS: usize = PORTS * NODES;
const INVENTORY: u32 = 0x167;
const PRODUCT: u32 = 0x197;
const VERSION: u32 = 0x19f;
const UNKNOWN: u32 = u32::MAX;

#[derive(Clone, Copy, Default)]
struct Device {
    address: u16,
    last_seen: u32,
    product: u32,
    version: u32,
    next_probe: u32,
}

impl Device {
    const EMPTY: Self = Self {
        address: 0,
        last_seen: 0,
        product: 0,
        version: 0,
        next_probe: 0,
    };
    fn present(&self, now: u32) -> bool {
        self.address != 0 && now.wrapping_sub(self.last_seen) <= 150
    }
}

#[derive(Default)]
struct Port {
    subnet: Option<u8>,
    devices: [Device; NODES],
}

pub struct Inventory {
    ports: [Port; PORTS],
    walk: [u16; SLOTS],
    cursor: usize,
    remaining: usize,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    pub const fn new() -> Self {
        Self {
            ports: [const {
                Port {
                    subnet: None,
                    devices: [Device::EMPTY; NODES],
                }
            }; PORTS],
            walk: [0; SLOTS],
            cursor: 0,
            remaining: 0,
        }
    }
}

fn node_index(node: u8) -> Option<usize> {
    (node & 31)
        .checked_sub(1)
        .map(usize::from)
        .filter(|&n| n < NODES)
}

fn block_byte(mask: u8, data: &[u8], index: usize) -> Option<u8> {
    let bit = 1u8 << index;
    if mask & bit == 0 {
        return None;
    }
    let span = 8 - mask.leading_zeros() as usize;
    let offset = if data.len() >= span {
        index
    } else if data.len() == mask.count_ones() as usize {
        (mask & (bit - 1)).count_ones() as usize
    } else {
        return None;
    };
    data.get(offset).copied()
}

pub enum Answer {
    Unhandled,
    Silent,
    Reply(Reply),
}

impl Inventory {
    /// Consume a CRC- and trailer-validated frame. Return an optional read-only
    /// identification request, never an application update or a write retry.
    pub fn observe(
        &mut self,
        port: usize,
        frame: &[u8],
        now: u32,
        can_probe: bool,
    ) -> Option<[u8; 11]> {
        let (&[a, b, c, d], data) = frame.split_first_chunk::<4>()?;
        let header = u32::from_be_bytes([a, b, c, d]);
        if data.len() != usize::from(d & 63) {
            return None;
        }
        let message = (header >> 20) & 0x3ff;
        let subnet = (header >> 12) as u8;
        let bus = self.ports.get_mut(port)?;
        if message == 0x3ff {
            if !data.is_empty() && bus.subnet.replace(subnet).is_some_and(|old| old != subnet) {
                bus.devices.fill(Device::default());
            }
            return None;
        }
        let node = node_index((header >> 6) as u8)?;
        let required = match message {
            0x22f => 11,
            0x25f => 5,
            0x26f => 4,
            0x2e7 => 3,
            0x323 => 6 + usize::from(*data.first()?),
            0x36f => 8,
            0x377 => 13,
            0x3e7 => 2,
            _ => return None,
        };
        if data.len() < required || *bus.subnet.get_or_insert(subnet) != subnet {
            return None;
        }
        if message == 0x3e7
            && data[0] == subnet
            && let Some(old) = node_index(data[1])
            && old != node
        {
            // At a new address do not inherit the identity of a displaced device.
            bus.devices[node] = core::mem::take(&mut bus.devices[old]);
        }
        let address = port as u16 * 1000 + u16::from(subnet & 7) * 100 + (NODES - node) as u16;
        let row = &mut bus.devices[node];
        if !row.present(now) {
            row.product = UNKNOWN;
            row.version = UNKNOWN;
            row.next_probe = now;
        }
        row.address = address;
        row.last_seen = now;
        if message == 0x2e7
            && let [0, 0, mask, values @ ..] = data
        {
            let get = |i| block_byte(*mask, values, i);
            if let (Some(x), Some(y), Some(z)) = (get(0), get(1), get(2)) {
                row.product = u32::from_le_bytes([x, y, z, 0]);
            }
            if let (Some(lo), Some(hi), Some(z)) = (get(3), get(4), get(5)) {
                row.version = u32::from(u16::from_le_bytes([lo, hi])) * 100 + u32::from(z);
            }
        }
        if can_probe
            && (row.product == UNKNOWN || row.version == UNKNOWN)
            && now.wrapping_sub(row.next_probe) < (1 << 31)
        {
            row.next_probe = now.wrapping_add(60);
            let mut body = [0x6d, 0x70, 0, 7, 0, 0, subnet, 0, 0, 0, 0];
            body[7..].copy_from_slice(&(1u32 << node).to_le_bytes());
            Some(body)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        for port in &mut self.ports {
            port.subnet = None;
            port.devices.fill(Device::default());
        }
        self.remaining = 0;
    }

    pub fn walking(&self) -> bool {
        self.remaining != 0
    }

    pub fn walk_next(&mut self) -> Option<Reply> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let address = if self.remaining == 0 {
            0
        } else {
            let value = self.walk[self.cursor];
            self.cursor += 1;
            value
        };
        Some(
            Call {
                service: 1,
                method: INVENTORY,
                body: &[],
            }
            .ok(&address.to_le_bytes()),
        )
    }

    pub fn handle(&mut self, call: &Call<'_>, now: u32) -> Answer {
        if !matches!(call.method, INVENTORY | PRODUCT | VERSION) {
            return Answer::Unhandled;
        }
        if call.method != INVENTORY && call.service != 1 {
            return Answer::Silent;
        }
        let Some((&address, tail)) = call.body.split_first_chunk::<2>() else {
            return Answer::Silent;
        };
        if !tail.is_empty() && (call.method == INVENTORY || tail.len() != 4) {
            return Answer::Silent;
        }
        let address = u16::from_le_bytes(address);
        if call.method == INVENTORY && self.remaining != 0 {
            return Answer::Reply(call.error(1));
        }
        let rows = self.ports.iter_mut().flat_map(|p| &mut p.devices);
        if call.method == INVENTORY && address == 0 {
            if call.service == 0 {
                for row in rows {
                    row.address = 0;
                }
                return Answer::Reply(call.ok(&[0, 0]));
            }
            self.cursor = 0;
            self.remaining = 1; // terminator, including for an empty inventory
            for row in rows.filter(|r| r.present(now)) {
                self.walk[self.remaining - 1] = row.address;
                self.remaining += 1;
            }
            return Answer::Reply(self.walk_next().unwrap());
        }
        let row = rows
            .into_iter()
            .find(|r| r.address == address && r.present(now));
        if call.method == INVENTORY {
            let result = row.map_or(u16::MAX, |r| {
                if call.service == 0 {
                    r.address = 0;
                }
                address
            });
            Answer::Reply(call.ok(&result.to_le_bytes()))
        } else {
            let Some(row) = row else {
                return Answer::Silent;
            };
            let value = if call.method == PRODUCT {
                row.product
            } else {
                row.version
            };
            let mut body = [0; 6];
            body[..2].copy_from_slice(&address.to_le_bytes());
            body[2..].copy_from_slice(&value.to_le_bytes());
            Answer::Reply(call.ok(&body))
        }
    }
}

#[cfg(test)]
mod tests;
