//! Normal-mode inventory. Bus addresses, not product numbers, identify entries.
use crate::trpc::{Call, GatewayVersion, Reply};
mod broadcast;
mod discovery;
mod groups;
mod outdoor;
pub use broadcast::Broadcast;
pub use discovery::{LineDiscovery, LineProbe};
mod settings;
mod telemetry;
use crate::tn4::tx::Request;
use settings::{Operation, Setting};
use telemetry::{Field, Telemetry};

const NODES: usize = 24;
const PORTS: usize = 5;
const SLOTS: usize = PORTS * NODES;
const INVENTORY: u32 = 0x167;
const PRODUCT: u32 = 0x197;
const VERSION: u32 = 0x19f;
const TAKING_ADDRESS: u32 = 0x15f;
const UNKNOWN: u32 = u32::MAX;

#[derive(Clone, Copy, Default)]
struct Device {
    address: u16,
    subnet: u8,
    last_seen: u32,
    product: u32,
    version: u32,
    identity_delay: u8,
    sensor_delay: u16,
    fan_delay: u8,
    rh_delay: u8,
    live: Telemetry,
}

impl Device {
    const EMPTY: Self = Self {
        address: 0,
        subnet: 0,
        last_seen: 0,
        product: 0,
        version: 0,
        identity_delay: 0,
        sensor_delay: 0,
        fan_delay: 0,
        rh_delay: 0,
        live: Telemetry::EMPTY,
    };
    fn present(&self, now: u32) -> bool {
        self.address != 0 && now.wrapping_sub(self.last_seen) <= 150
    }
}

#[derive(Default)]
struct Port {
    link: LineDiscovery,
    subnet: Option<u8>,
    devices: [Device; NODES],
    pending: u32,
    job: Operation,
}

pub struct Inventory {
    groups: groups::Groups,
    broadcasts: broadcast::Broadcasts,
    outdoor: outdoor::Outdoor,
    ports: [Port; PORTS],
    walk: [u16; SLOTS],
    cursor: usize,
    remaining: usize,
    reporting: bool,
    setbacks: bool,
    network_error: u16,
    error_pending: bool,
    next_slot: usize,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    pub const fn new() -> Self {
        Self {
            groups: groups::Groups::EMPTY,
            broadcasts: broadcast::Broadcasts::EMPTY,
            outdoor: outdoor::Outdoor::EMPTY,
            ports: [const {
                Port {
                    link: LineDiscovery::COLD,
                    subnet: None,
                    devices: [Device::EMPTY; NODES],
                    pending: 0,
                    job: Operation::IDLE,
                }
            }; PORTS],
            walk: [0; SLOTS],
            cursor: 0,
            remaining: 0,
            reporting: false,
            setbacks: false,
            network_error: 0,
            error_pending: false,
            next_slot: 0,
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
    let offset = if data.len() >= 8 || (mask >> data.len()) == 0 {
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

pub struct AddressMove {
    old: u16,
    new: u16,
}

impl AddressMove {
    pub fn report(self) -> Reply {
        let [a, b] = self.old.to_le_bytes();
        let [c, d] = self.new.to_le_bytes();
        Reply::report(TAKING_ADDRESS, &[a, b, c, d])
    }
}

pub enum Observation {
    Probe([u8; 11]),
    Moved(AddressMove),
}

impl Inventory {
    /// Consume a validated native frame. Never initiate an application write.
    pub fn observe(
        &mut self,
        port: usize,
        frame: &[u8],
        now: u32,
        can_probe: bool,
    ) -> Option<Observation> {
        let (&[a, b, c, d], data) = frame.split_first_chunk::<4>()?;
        let header = u32::from_be_bytes([a, b, c, d]);
        if data.len() != usize::from(d & 63) {
            return None;
        }
        let message = (header >> 20) & 0x3ff;
        let subnet = (header >> 12) as u8;
        let bus = self.ports.get_mut(port)?;
        bus.link.peer(message);
        if message == 0x32f {
            self.outdoor
                .observe(data, now, self.reporting, &mut self.broadcasts);
            return None;
        }
        if message == 0x327 && data.is_empty() {
            self.broadcasts.push(
                broadcast::Value::RequestedOutdoor(self.outdoor.value()),
                now,
            );
            return None;
        }
        if message == 0x20f {
            self.groups.observe(data, now, self.reporting);
            return None;
        }
        if message == 0x3d7 {
            let [lo, hi, ..] = data else { return None };
            self.network_error = u16::from_le_bytes([*lo, *hi]);
            self.error_pending = self.reporting;
            return None;
        }
        if message == 0x3ff {
            if bus.subnet.replace(subnet).is_some_and(|old| old != subnet) {
                if bus.job.active() {
                    bus.job.fail();
                }
                bus.devices.fill(Device::default());
                bus.pending = 0;
            }
            return None;
        }
        let node = node_index((header >> 6) as u8)?;
        let address = port as u16 * 1000 + u16::from(subnet & 7) * 100 + (NODES - node) as u16;
        if message == 0x3e7 {
            let [old_subnet, old_node, ..] = data else {
                return None;
            };
            let old = node_index(*old_node)?;
            let previous = &bus.devices[old];
            if !previous.present(now) || previous.subnet != *old_subnet {
                return None;
            }
            let moved = AddressMove {
                old: previous.address,
                new: address,
            };
            if bus.job.active() && [moved.old, bus.devices[node].address].contains(&bus.job.address)
            {
                bus.job.fail();
            }
            if old != node {
                bus.devices[node] = core::mem::take(&mut bus.devices[old]);
            }
            bus.pending &= !(1 << old | 1 << node);
            let row = &mut bus.devices[node];
            row.live.dirty = 0;
            row.address = address;
            row.subnet = subnet;
            row.last_seen = now;
            return Some(Observation::Moved(moved));
        }
        let required = match message {
            0x22f => 11,
            0x25f => 5,
            0x26f => 4,
            0x2e7 => 3,
            0x2f7 if bus.devices[node].present(now) => 3,
            0x323 => 6 + usize::from(*data.first()?),
            0x36f => 8,
            0x377 => 13,
            _ => return None,
        };
        if data.len() < required {
            return None;
        }
        let row = &mut bus.devices[node];
        let present = row.present(now);
        let expected_subnet = if present {
            row.subnet
        } else {
            *bus.subnet.get_or_insert(subnet)
        };
        if expected_subnet != subnet {
            return None;
        }
        if !present {
            if bus.job.active() && bus.job.address == address {
                bus.job.fail();
            }
            row.product = UNKNOWN;
            row.version = UNKNOWN;
            row.identity_delay = 0;
            row.sensor_delay = 0;
            row.fan_delay = 0;
            row.rh_delay = 0;
            row.live = Telemetry::UNKNOWN;
            bus.pending &= !(1 << node);
        } else {
            // A present row was observed within 150 seconds, including at clock wrap.
            let elapsed = now.wrapping_sub(row.last_seen) as u8;
            row.identity_delay = row.identity_delay.saturating_sub(elapsed);
            row.sensor_delay = row.sensor_delay.saturating_sub(u16::from(elapsed));
            row.fan_delay = row.fan_delay.saturating_sub(elapsed);
            row.rh_delay = row.rh_delay.saturating_sub(elapsed);
        }
        row.address = address;
        row.subnet = subnet;
        row.last_seen = now;
        let previous_attributes = row.live.attributes;
        if message == 0x22f && data[1] & 0x30 == 0x10
            || message == 0x2e7
                && matches!(data, [0x26 | 0x28 | 0x2a | 0x2c | 0x2e | 0x30, 0, _, _, ..])
        {
            row.live.mark_setpoint_device();
        }
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
        let mut changed = if message == 0x2e7
            && let [13, 0, mask, values @ ..] = data
            && let Some(changed) = row.live.sensor_setup(*mask, values)
        {
            row.sensor_delay = 3600;
            changed
        } else if message == 0x2e7
            && (!row.live.is_setpoint_device() || !matches!(data, [3 | 5 | 6, 0, ..]))
        {
            row.live.setting_report(data)
        } else {
            row.live.observe(message, data)
        };
        if bus.job.active() && bus.job.address == address {
            if row.live.is_setpoint_device()
                && matches!(
                    bus.job.setting,
                    Setting::Mode | Setting::Heat | Setting::Cool
                )
                || matches!(bus.job.setting, Setting::Heat | Setting::Cool)
                    && !row.live.configured(bus.job.setting)
            {
                bus.job.fail();
            } else if message == 0x2e7 {
                bus.job.sample(address, subnet, data, &row.live);
            }
        }
        if !self.setbacks {
            changed &= !Field::Setback.mask();
        }
        if row.live.attributes != previous_attributes {
            changed |= Field::Attributes.mask();
        }
        if self.reporting && changed != 0 {
            row.live.dirty |= changed;
            bus.pending |= 1 << node;
        }
        if !can_probe {
            return None;
        }
        let block = if (row.product == UNKNOWN || row.version == UNKNOWN) && row.identity_delay == 0
        {
            row.identity_delay = 60;
            0
        } else if message == 0x26f && row.sensor_delay == 0 {
            row.sensor_delay = 60;
            13
        } else if message == 0x25f && row.fan_delay == 0 {
            row.fan_delay = 60;
            9
        } else if message == 0x323 && row.rh_delay == 0 {
            row.rh_delay = 60;
            0xa3
        } else if message == 0x2e7
            && matches!(data, [0xa3, 0, ..])
            && row.live.configured(Setting::HumidityMin)
        {
            0xa4
        } else if message == 0x2e7
            && matches!(data, [0xa3 | 0xa4, 0, ..])
            && row.live.configured(Setting::HumidityMax)
        {
            0xa5
        } else {
            return None;
        };
        let mut body = [0x6d, 0x70, 0, 7, block, 0, subnet, 0, 0, 0, 0];
        body[7..].copy_from_slice(&(1u32 << node).to_le_bytes());
        Some(Observation::Probe(body))
    }

    pub fn reset(&mut self) {
        self.groups = groups::Groups::EMPTY;
        self.broadcasts = broadcast::Broadcasts::EMPTY;
        self.outdoor = outdoor::Outdoor::EMPTY;
        for port in &mut self.ports {
            port.link = LineDiscovery::COLD;
            port.subnet = None;
            port.devices.fill(Device::default());
            port.pending = 0;
            port.job = Operation::IDLE;
        }
        self.remaining = 0;
        self.error_pending = false;
        self.network_error = 0;
    }

    pub fn has_command(&self, p: usize) -> bool {
        self.ports[p].job.active()
    }
    pub fn tick(&mut self, now: u32) {
        for port in &mut self.ports {
            port.link.tick(now);
        }
        self.outdoor.tick(now, self.reporting, &mut self.broadcasts);
    }
    pub fn line(&mut self, p: usize) -> &mut LineDiscovery {
        &mut self.ports[p].link
    }
    pub fn has_broadcast(&self, p: usize) -> bool {
        self.broadcasts.pending(p)
    }
    pub fn next_broadcast(&mut self, p: usize, now: u32) -> Option<Broadcast> {
        while let Some(tx) = self.broadcasts.take(p, now) {
            if !broadcast::Broadcasts::stale_outdoor(tx, &self.outdoor) {
                return Some(tx);
            }
        }
        None
    }
    pub fn stale_broadcast(&self, p: usize, tx: Broadcast, now: u32) -> bool {
        self.broadcasts.stale(p, tx, now) || broadcast::Broadcasts::stale_outdoor(tx, &self.outdoor)
    }
    pub fn command_finished(&self, p: usize) -> bool {
        self.ports[p].job.terminal()
    }
    pub fn command_expire(&mut self, p: usize, now: u32) {
        self.ports[p].job.expire(now);
    }
    pub fn command_frame(&mut self, p: usize) -> Option<Request> {
        self.ports[p].job.take_frame(p as u8)
    }
    pub fn command_sent(&mut self, p: usize, complete: bool) {
        self.ports[p].job.sent(complete);
    }
    pub fn command_reply(&mut self, p: usize) -> Option<Reply> {
        let slot = &mut self.ports[p].job;
        let reply = slot.reply()?;
        *slot = Operation::IDLE;
        Some(reply)
    }

    pub fn set_reporting(&mut self, on: bool) {
        self.reporting = on;
        if !on {
            self.groups.clear_reports();
            self.outdoor.dirty = false;
            self.error_pending = false;
            self.discard_fields(u16::MAX);
        }
    }

    fn discard_fields(&mut self, mask: u16) {
        for port in &mut self.ports {
            let mut pending = port.pending;
            while pending != 0 {
                let i = pending.trailing_zeros() as usize;
                pending &= pending - 1;
                port.devices[i].live.dirty &= !mask;
                if port.devices[i].live.dirty == 0 {
                    port.pending &= !(1 << i);
                }
            }
        }
    }
    pub fn has_reports(&self) -> bool {
        self.error_pending
            || self.outdoor.dirty
            || self.groups.has_reports()
            || self.ports.iter().any(|p| p.pending != 0)
    }
    pub fn next_report(&mut self, now: u32) -> Option<Reply> {
        if core::mem::take(&mut self.error_pending) {
            return Some(Reply::report(0x107, &self.network_error.to_le_bytes()));
        }
        if let Some(report) = self.outdoor.report() {
            return Some(report);
        }
        if let Some(report) = self.groups.next_report(now) {
            return Some(report);
        }
        for _ in 0..SLOTS {
            let i = self.next_slot;
            self.next_slot = if i + 1 == SLOTS { 0 } else { i + 1 };
            let p = &mut self.ports[i / NODES];
            let node = i % NODES;
            if p.pending & (1 << node) == 0 {
                continue;
            }
            let row = &mut p.devices[node];
            let r = if row.present(now) {
                row.live.report(row.address)
            } else {
                row.live.dirty = 0;
                None
            };
            if row.live.dirty == 0 {
                p.pending &= !(1 << node);
            }
            if r.is_some() {
                return r;
            }
        }
        None
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
        if let Some(version) = GatewayVersion::from_method(call.method) {
            return if call.service == 1 {
                Answer::Reply(call.ok(&version.word()))
            } else {
                Answer::Silent
            };
        }
        if call.method == 0x1a7 {
            if call.service != 0 {
                return Answer::Silent;
            }
            // J1196J checks native TX mailbox depth, not an active driver or pending serial reply.
            if let Some(value) = broadcast::Value::clock(call.body)
                && self.broadcasts.is_empty()
                && !self.ports.iter().any(|p| p.job.frame_waiting())
            {
                self.broadcasts.push(value, now);
                return Answer::Reply(call.ok(call.body));
            }
            return Answer::Reply(call.ok(&[0xff; 7]));
        }
        if call.method == groups::METHOD {
            return self.groups.handle(call, now, &mut self.broadcasts);
        }
        if call.method == outdoor::METHOD {
            return self
                .outdoor
                .handle(call, now, self.reporting, &mut self.broadcasts);
        }
        if let Some(setting) = Setting::from_method(call.method) {
            let Some((&a, _)) = call.body.split_first_chunk::<2>() else {
                return Answer::Silent;
            };
            let address = u16::from_le_bytes(a);
            let Some(bus) = self.ports.get_mut(usize::from(address / 1000)) else {
                return Answer::Silent;
            };
            let Some((i, row)) = bus
                .devices
                .iter()
                .enumerate()
                .find(|(_, r)| r.address == address && r.present(now))
            else {
                return Answer::Silent;
            };
            let Some(mut op) = Operation::begin(
                call,
                setting,
                row.subnet,
                i as u8 + 1,
                row.live.setback(),
                self.setbacks,
                now,
            ) else {
                return Answer::Silent;
            };
            if row.live.is_setpoint_device()
                && matches!(setting, Setting::Mode | Setting::Heat | Setting::Cool)
                || matches!(
                    setting,
                    Setting::Heat | Setting::Cool | Setting::DeviceTarget
                ) && !row.live.configured(setting)
                || matches!(setting, Setting::HumidityMin | Setting::HumidityMax)
                    && !row.live.has_rh()
                || setting == Setting::Fan
                    && matches!(row.product, 99201..=99203)
                    && call.service == 0
                    && call.body.last().is_some_and(|v| *v > 10)
            {
                op.fail();
            }
            return settings::accept(&mut bus.job, op, call);
        }
        match call.method {
            0x10f | 0x16f => {
                match (call.service, call.body) {
                    (0, [v]) if call.method == 0x10f => self.set_reporting(*v != 0),
                    (0, [v]) => {
                        self.setbacks = *v != 0;
                        if !self.setbacks {
                            self.discard_fields(Field::Setback.mask());
                        }
                    }
                    (1, [] | [_]) => {}
                    _ => return Answer::Silent,
                }
                return Answer::Reply(call.ok(&[u8::from(if call.method == 0x10f {
                    self.reporting
                } else {
                    self.setbacks
                })]));
            }
            0x107 if matches!(call.body, [_, _]) || call.service == 1 && call.body.is_empty() => {
                return Answer::Reply(call.ok(&self.network_error.to_le_bytes()));
            }
            0x107 => return Answer::Silent,
            _ => {}
        }
        let field = Field::from_method(call.method);
        if !matches!(call.method, INVENTORY | PRODUCT | VERSION) && field.is_none() {
            return Answer::Unhandled;
        }
        if (matches!(call.method, PRODUCT | VERSION) || field == Some(Field::Humidity))
            && call.service != 1
        {
            return Answer::Silent;
        }
        let Some((&address, tail)) = call.body.split_first_chunk::<2>() else {
            return Answer::Silent;
        };
        if call.service == 0 && field.is_some() && tail.is_empty()
            || !tail.is_empty()
                && (call.method == INVENTORY || tail.len() != field.map_or(4, Field::width))
        {
            return Answer::Silent;
        }
        let address = u16::from_le_bytes(address);
        if call.method == INVENTORY && self.remaining != 0 {
            return Answer::Reply(call.error(1));
        }
        if call.method == INVENTORY && call.service == 0 {
            for p in &mut self.ports {
                if p.job.active() && (address == 0 || address == p.job.address) {
                    p.job.fail();
                }
            }
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
            if let Some(field) = field {
                return Answer::Reply(row.live.reply(address, field, call));
            }
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
