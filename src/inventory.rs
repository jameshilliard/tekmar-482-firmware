//! Normal-mode inventory. Bus addresses, not product numbers, identify entries.
use crate::tn4::{self, block, message};
use crate::trpc::{BridgeError, Call, GatewayVersion, Reply, method, service};
mod air;
mod broadcast;
mod codes;
mod groups;
mod line;
mod outdoor;
mod reads;
pub use broadcast::Broadcast;
pub use line::{LineProbe, NormalLine};
pub use reads::BlockProbe;
mod settings;
mod telemetry;
use crate::tn4::tx::{Outcome, Request};
use settings::{Operation, Setting};
use telemetry::{Field, Telemetry};

const NODES: usize = 24;
const PORTS: usize = tn4::PORT_COUNT;
const SLOTS: usize = PORTS * NODES;
use method::{
    DEVICE_INVENTORY as INVENTORY, DEVICE_TYPE as PRODUCT, DEVICE_VERSION as VERSION,
    TAKING_ADDRESS,
};
const ADDRESS_PORT_SCALE: u16 = 1000;
const ADDRESS_BUS_SCALE: u16 = 100;
const WATER_SUBNET_BUS_MASK: u8 = 7;
const DEVICE_TTL_SECONDS: u32 = 150;
const PROBE_RETRY_SECONDS: u8 = 60;
const SETPOINT_SETUP_SECONDS: u16 = 300;
const MODE_REFRESH_SECONDS: u16 = 300;
const SENSOR_SETUP_SECONDS: u16 = 3600;
const STEPPED_FAN_PRODUCTS: core::ops::RangeInclusive<u32> = 99201..=99203;
const MAX_FAN_STEP: u8 = 10;
const IDENTITY_Z_RADIX: u32 = 100;
const BLOCK_REQUEST_PAYLOAD_BYTES: usize = 2 + 1 + size_of::<u32>();
const BLOCK_REQUEST_FRAME_BYTES: usize = tn4::HEADER_BYTES + BLOCK_REQUEST_PAYLOAD_BYTES;
const UNKNOWN: u32 = u32::MAX;

#[derive(Clone, Copy, Default)]
struct Device {
    address: u16,
    subnet: u8,
    last_seen: u32,
    product: u32,
    version: u32,
    incarnation: u16,
    identity_delay: u8,
    mode_delay: u16,
    sensor_delay: u16,
    setpoint_setup_delay: u16,
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
        incarnation: 0,
        identity_delay: 0,
        mode_delay: 0,
        sensor_delay: 0,
        setpoint_setup_delay: 0,
        fan_delay: 0,
        rh_delay: 0,
        live: Telemetry::EMPTY,
    };
    fn present(&self, now: u32) -> bool {
        self.address != 0 && now.wrapping_sub(self.last_seen) <= DEVICE_TTL_SECONDS
    }
}

#[derive(Default)]
struct Port {
    link: NormalLine,
    reads: reads::Reads,
    subnet: Option<u8>,
    devices: [Device; NODES],
    pending: u32,
    job: Operation,
}

pub struct Inventory {
    air_groups: air::AirGroups,
    groups: groups::Groups,
    broadcasts: broadcast::Broadcasts,
    outdoor: outdoor::Outdoor,
    incarnation: u16,
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
            air_groups: air::AirGroups::EMPTY,
            groups: groups::Groups::EMPTY,
            broadcasts: broadcast::Broadcasts::EMPTY,
            outdoor: outdoor::Outdoor::EMPTY,
            incarnation: 0,
            ports: [const {
                Port {
                    link: NormalLine::COLD,
                    reads: reads::Reads::EMPTY,
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
    (node & tn4::NODE_MASK)
        .checked_sub(1)
        .map(usize::from)
        .filter(|&n| n < NODES)
}

fn block_request(block: u8, subnet: u8, node_index: usize) -> [u8; BLOCK_REQUEST_FRAME_BYTES] {
    let mut frame = [0; BLOCK_REQUEST_FRAME_BYTES];
    frame[..tn4::HEADER_BYTES].copy_from_slice(&tn4::gateway_header(
        message::BLOCK_REQUEST,
        BLOCK_REQUEST_PAYLOAD_BYTES,
    ));
    let payload = &mut frame[tn4::HEADER_BYTES..];
    payload[0] = block;
    payload[2] = subnet;
    payload[3..].copy_from_slice(&(1u32 << node_index).to_le_bytes());
    frame
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

impl Inventory {
    /// Consume a validated native frame. Never initiate an application write.
    pub fn observe(&mut self, port: usize, frame: &[u8], now: u32) -> Option<AddressMove> {
        let (&[a, b, c, d], data) = frame.split_first_chunk::<4>()?;
        let header = u32::from_be_bytes([a, b, c, d]);
        if data.len() != usize::from(d & tn4::PAYLOAD_MASK) {
            return None;
        }
        let message = (header >> tn4::MESSAGE_SHIFT) & tn4::MESSAGE_MASK;
        let subnet = (header >> tn4::SUBNET_SHIFT) as u8;
        let bus = self.ports.get_mut(port)?;
        bus.link.peer(message);
        if message == message::WATER_CYCLE_START {
            // Native plant telemetry, not a tHA report or a bus lock.
            return None;
        }
        if message == message::OUTDOOR_TEMPERATURE_REPORT {
            self.outdoor
                .observe(data, now, self.reporting, &mut self.broadcasts);
            return None;
        }
        if message == message::OUTDOOR_TEMPERATURE_REQUEST && data.is_empty() {
            self.broadcasts.push(
                broadcast::Value::RequestedOutdoor(self.outdoor.value()),
                now,
            );
            return None;
        }
        if message == message::SETPOINT_ENABLE_REPORT {
            self.groups.observe(data, now, self.reporting);
            return None;
        }
        if message == message::NETWORK_ERROR {
            let [lo, hi, ..] = data else { return None };
            self.network_error = u16::from_le_bytes([*lo, *hi]);
            self.error_pending = self.reporting;
            return None;
        }
        if message == message::MASTER_PRESENT {
            if bus.subnet.replace(subnet).is_some_and(|old| old != subnet) {
                if bus.job.active() {
                    bus.job.fail();
                }
                bus.devices.fill(Device::default());
                bus.pending = 0;
                bus.reads = reads::Reads::EMPTY;
            }
            return None;
        }
        let node = node_index((header >> tn4::NODE_SHIFT) as u8)?;
        let address = port as u16 * ADDRESS_PORT_SCALE
            + u16::from(subnet & WATER_SUBNET_BUS_MASK) * ADDRESS_BUS_SCALE
            + (NODES - node) as u16;
        if message == message::TAKING_ADDRESS {
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
            bus.reads.forget(old);
            bus.reads.forget(node);
            self.incarnation = self.incarnation.wrapping_add(1);
            bus.pending &= !(1 << old | 1 << node);
            let row = &mut bus.devices[node];
            row.incarnation = self.incarnation;
            row.live.dirty = 0;
            row.address = address;
            row.subnet = subnet;
            row.last_seen = now;
            return Some(moved);
        }
        let required = match message {
            message::SETPOINT_STATUS => message::payload_bytes::SETPOINT_STATUS,
            message::FAN_DUCT_STATUS => message::payload_bytes::FAN_DUCT_STATUS,
            message::SLAB_STATUS => message::payload_bytes::SLAB_STATUS,
            message::BLOCK_REPORT => message::payload_bytes::BLOCK_REPORT_PREFIX,
            message::SCHEDULE_STATUS if bus.devices[node].present(now) => {
                message::payload_bytes::SCHEDULE_STATUS
            }
            message::RH_REPORT => {
                message::payload_bytes::RH_REPORT_PREFIX + usize::from(*data.first()?)
            }
            message::AIR_GROUP_MEMBER_STATUS => message::payload_bytes::AIR_GROUP_MEMBER_STATUS,
            message::AIR_GROUP_MASTER_STATUS => message::payload_bytes::AIR_GROUP_MASTER_STATUS,
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
            bus.reads.forget(node);
            self.incarnation = self.incarnation.wrapping_add(1);
            *row = Device {
                incarnation: self.incarnation,
                product: UNKNOWN,
                version: UNKNOWN,
                live: Telemetry::UNKNOWN,
                ..Device::EMPTY
            };
            bus.pending &= !(1 << node);
        } else {
            // A present row was observed within 150 seconds, including at clock wrap.
            let elapsed = now.wrapping_sub(row.last_seen) as u8;
            row.identity_delay = row.identity_delay.saturating_sub(elapsed);
            row.mode_delay = row.mode_delay.saturating_sub(u16::from(elapsed));
            row.sensor_delay = row.sensor_delay.saturating_sub(u16::from(elapsed));
            row.setpoint_setup_delay = row.setpoint_setup_delay.saturating_sub(u16::from(elapsed));
            row.fan_delay = row.fan_delay.saturating_sub(elapsed);
            row.rh_delay = row.rh_delay.saturating_sub(elapsed);
        }
        row.address = address;
        row.subnet = subnet;
        row.last_seen = now;
        let previous_attributes = row.live.attributes;
        let prior_hint = matches!(
            message,
            message::AIR_GROUP_MEMBER_STATUS | message::AIR_GROUP_MASTER_STATUS
        )
        .then(|| row.live.mode_hint());
        if message == message::BLOCK_REPORT
            && let [id, 0, _, ..] = data
        {
            bus.reads.reported(node, *id);
        }
        let setpoint_running = message == message::SETPOINT_STATUS
            && data[1] & codes::setpoint::STATUS_MODE_MASK == codes::setpoint::STATUS_RUNNING;
        if setpoint_running
            || message == message::BLOCK_REPORT
                && matches!(
                    data,
                    [
                        block::SETPOINT_SETUP
                            | block::SETPOINT_TARGETS
                            | block::HOT_TUB_TARGETS
                            | block::DHW_TARGETS
                            | block::FLOOR_TARGETS
                            | block::POOL_TARGETS,
                        0,
                        _,
                        _,
                        ..
                    ]
                )
        {
            row.live.mark_setpoint_device();
        }
        if message == message::BLOCK_REPORT
            && let [block::SETPOINT_SETUP, 0, mask, values @ ..] = data
            && let Some(app) = block_byte(*mask, values, 0)
        {
            row.setpoint_setup_delay = if Setting::DeviceTarget.block(Some(app)).is_some() {
                SETPOINT_SETUP_SECONDS
            } else {
                0
            };
            bus.reads.discard(node, block::SETPOINT_SETUP);
        }
        if message == message::BLOCK_REPORT
            && let [block::DEVICE_INFO, 0, mask, values @ ..] = data
        {
            let get = |i| block_byte(*mask, values, i);
            if let (Some(x), Some(y), Some(z)) = (get(0), get(1), get(2)) {
                row.product = u32::from_le_bytes([x, y, z, 0]);
            }
            if let (Some(lo), Some(hi), Some(z)) = (get(3), get(4), get(5)) {
                row.version =
                    u32::from(u16::from_le_bytes([lo, hi])) * IDENTITY_Z_RADIX + u32::from(z);
            }
        }
        let mut changed = if message == message::BLOCK_REPORT
            && let [block::SENSOR_SETUP, 0, mask, values @ ..] = data
            && let Some(changed) = row.live.sensor_setup(*mask, values)
        {
            row.sensor_delay = SENSOR_SETUP_SECONDS;
            bus.reads.discard(node, block::SENSOR_SETUP);
            changed
        } else if message == message::BLOCK_REPORT {
            row.live.setting_report(data)
        } else if let Some(air) = self.air_groups.observe(message, data) {
            row.live.air_status(air, self.setbacks)
        } else {
            row.live.observe(message, data)
        };
        if bus.job.active() && bus.job.address == address {
            if matches!(
                bus.job.setting,
                Setting::Mode | Setting::Heat | Setting::Cool
            ) && !row.live.configured(bus.job.setting)
            {
                bus.job.fail();
            } else if message == message::BLOCK_REPORT {
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
        if row.setpoint_setup_delay == 0
            && (setpoint_running
                || message == message::BLOCK_REPORT
                    && data.starts_with(&[block::DEVICE_INFO, 0])
                    && row.live.is_setpoint_device())
        {
            bus.reads.queue(node, block::SETPOINT_SETUP);
            row.setpoint_setup_delay = u16::from(PROBE_RETRY_SECONDS);
        }
        if message == message::SLAB_STATUS && row.sensor_delay == 0 {
            bus.reads.queue(node, block::SENSOR_SETUP);
            row.sensor_delay = u16::from(PROBE_RETRY_SECONDS);
        }
        if (row.product == UNKNOWN || row.version == UNKNOWN) && row.identity_delay == 0 {
            bus.reads.queue(node, block::DEVICE_INFO);
            row.identity_delay = PROBE_RETRY_SECONDS;
        }
        if let Some(before) = prior_hint {
            for id in [block::HEAT_SETPOINTS, block::COOL_SETPOINTS] {
                if row.live.missing_target(id) {
                    bus.reads.queue(node, id);
                }
            }
            if row.live.configured(Setting::Mode)
                && (row.mode_delay == 0 || before != row.live.mode_hint())
            {
                bus.reads.queue(node, block::HEATING_SETUP);
                row.mode_delay = if row.live.mode_known() {
                    MODE_REFRESH_SECONDS
                } else {
                    u16::from(PROBE_RETRY_SECONDS)
                };
            }
        }
        if message == message::BLOCK_REPORT
            && let [block::HEATING_SETUP, 0, mask, values @ ..] = data
            && block_byte(*mask, values, 0).is_some()
            && row.live.mode_known()
        {
            row.mode_delay = MODE_REFRESH_SECONDS;
            bus.reads.discard(node, block::HEATING_SETUP);
        }
        if message == message::SLAB_STATUS && row.live.missing_target(block::SLAB_SETPOINTS) {
            bus.reads.queue(node, block::SLAB_SETPOINTS);
        }
        if message == message::FAN_DUCT_STATUS
            && row.fan_delay == 0
            && row.live.missing_target(block::FAN_PERCENT)
        {
            bus.reads.queue(node, block::FAN_PERCENT);
            row.fan_delay = PROBE_RETRY_SECONDS;
        }
        if message == message::RH_REPORT && row.rh_delay == 0 {
            bus.reads.queue(node, block::HUMIDITY_SETUP);
            row.rh_delay = PROBE_RETRY_SECONDS;
        }
        if message == message::RH_REPORT
            || message == message::BLOCK_REPORT && data.starts_with(&[block::HUMIDITY_SETUP, 0])
        {
            for id in [block::HUMIDIFY_TARGETS, block::DEHUMIDIFY_TARGETS] {
                if row.live.missing_target(id) {
                    bus.reads.queue(node, id);
                }
            }
        }
        None
    }

    pub fn reset(&mut self) {
        self.air_groups = air::AirGroups::EMPTY;
        self.groups = groups::Groups::EMPTY;
        self.broadcasts = broadcast::Broadcasts::EMPTY;
        self.outdoor = outdoor::Outdoor::EMPTY;
        for port in &mut self.ports {
            port.reads = reads::Reads::EMPTY;
            port.link = NormalLine::COLD;
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
    pub fn line(&mut self, p: usize) -> &mut NormalLine {
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
    pub fn command_sent(&mut self, p: usize, outcome: Outcome) {
        self.ports[p].job.sent(outcome);
    }
    pub fn command_unavailable(&mut self, p: usize) {
        self.ports[p].job.fail();
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
            return Some(Reply::report(
                method::NETWORK_ERROR,
                &self.network_error.to_le_bytes(),
            ));
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
                service: service::REQUEST,
                method: INVENTORY,
                body: &[],
            }
            .ok(&address.to_le_bytes()),
        )
    }

    pub fn handle(&mut self, call: &Call<'_>, now: u32) -> Answer {
        if let Some(version) = GatewayVersion::from_method(call.method) {
            return if call.service == service::REQUEST {
                Answer::Reply(call.ok(&version.word()))
            } else {
                Answer::Silent
            };
        }
        if call.method == method::DATE_TIME {
            if call.service != service::UPDATE {
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
            return Answer::Reply(call.ok(&[codes::UNAVAILABLE; broadcast::THA_CLOCK_BYTES]));
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
            let Some(bus) = self
                .ports
                .get_mut(usize::from(address / ADDRESS_PORT_SCALE))
            else {
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
            if matches!(
                setting,
                Setting::Mode | Setting::Heat | Setting::Cool | Setting::DeviceTarget
            ) && !row.live.configured(setting)
                || matches!(setting, Setting::HumidityMin | Setting::HumidityMax)
                    && !row.live.has_rh()
                || setting == Setting::Fan
                    && STEPPED_FAN_PRODUCTS.contains(&row.product)
                    && call.service == service::UPDATE
                    && call.body.last().is_some_and(|v| *v > MAX_FAN_STEP)
            {
                op.fail();
            }
            return settings::accept(&mut bus.job, op, call);
        }
        match call.method {
            method::REPORTING_STATE | method::SETBACK_ENABLE => {
                match (call.service, call.body) {
                    (service::UPDATE, [v]) if call.method == method::REPORTING_STATE => {
                        self.set_reporting(*v != 0)
                    }
                    (service::UPDATE, [v]) => {
                        self.setbacks = *v != 0;
                        if !self.setbacks {
                            self.discard_fields(Field::Setback.mask());
                        }
                    }
                    (service::REQUEST, [] | [_]) => {}
                    _ => return Answer::Silent,
                }
                return Answer::Reply(call.ok(&[u8::from(
                    if call.method == method::REPORTING_STATE {
                        self.reporting
                    } else {
                        self.setbacks
                    },
                )]));
            }
            method::NETWORK_ERROR
                if matches!(call.body, [_, _])
                    || call.service == service::REQUEST && call.body.is_empty() =>
            {
                return Answer::Reply(call.ok(&self.network_error.to_le_bytes()));
            }
            method::NETWORK_ERROR => return Answer::Silent,
            _ => {}
        }
        let field = Field::from_method(call.method);
        if !matches!(call.method, INVENTORY | PRODUCT | VERSION) && field.is_none() {
            return Answer::Unhandled;
        }
        if (matches!(call.method, PRODUCT | VERSION) || field == Some(Field::Humidity))
            && call.service != service::REQUEST
        {
            return Answer::Silent;
        }
        let Some((&address, tail)) = call.body.split_first_chunk::<2>() else {
            return Answer::Silent;
        };
        if call.service == service::UPDATE && field.is_some() && tail.is_empty()
            || !tail.is_empty()
                && (call.method == INVENTORY || tail.len() != field.map_or(4, Field::width))
        {
            return Answer::Silent;
        }
        let address = u16::from_le_bytes(address);
        if call.method == INVENTORY && self.remaining != 0 {
            return Answer::Reply(call.error(BridgeError::Busy));
        }
        if call.method == INVENTORY && call.service == service::UPDATE {
            for p in &mut self.ports {
                if p.job.active() && (address == 0 || address == p.job.address) {
                    p.job.fail();
                }
            }
        }
        let rows = self.ports.iter_mut().flat_map(|p| &mut p.devices);
        if call.method == INVENTORY && address == 0 {
            if call.service == service::UPDATE {
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
                if call.service == service::UPDATE {
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
