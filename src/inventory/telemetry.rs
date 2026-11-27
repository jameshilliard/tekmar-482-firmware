//! Device readings common to the tHA Request, Response:Update and Report paths.
use super::codes::{TEMP_FAULT_WORDS, mode, setback, setpoint};
use super::{
    air::{AirStatus, Side},
    block_byte,
    settings::{Setting, slots},
};
use crate::tn4::{block, message};
use crate::trpc::{Call, Reply, method};
const HEAT_REPORT: usize = Side::Heat as usize;
const COOL_REPORT: usize = Side::Cool as usize;
const SLAB_REPORT: usize = 2;
const FAN_REPORT: usize = 3;
use super::codes::{UNAVAILABLE, demand};
const SENSOR_SLOTS: usize = 5;
const SENSOR_ROLE_OFF: u8 = 0;
const SENSOR_ROLE_FLOOR: u8 = 2;
const SENSOR_MASK: u8 = (1 << SENSOR_SLOTS) - 1;
const EVENT_COUNT_SHIFT: u8 = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    Attributes,
    Temperature,
    Floor,
    Demand,
    Humidity,
    Setback,
    Mode,
    Heat,
    Cool,
    SlabTarget,
    Fan,
    HumidityMin,
    HumidityMax,
    Events,
    DeviceTarget,
}
impl Field {
    const ALL: [Self; 15] = [
        Self::Attributes,
        Self::Temperature,
        Self::Floor,
        Self::Demand,
        Self::Humidity,
        Self::Setback,
        Self::Mode,
        Self::Heat,
        Self::Cool,
        Self::SlabTarget,
        Self::Fan,
        Self::HumidityMin,
        Self::HumidityMax,
        Self::Events,
        Self::DeviceTarget,
    ];
    pub(super) fn from_method(method: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.method() == method)
    }
    fn method(self) -> u32 {
        match self {
            Self::Attributes => method::DEVICE_ATTRIBUTES,
            Self::Temperature => method::CURRENT_TEMPERATURE,
            Self::Floor => method::CURRENT_FLOOR_TEMPERATURE,
            Self::Demand => method::ACTIVE_DEMAND,
            Self::Humidity => method::RELATIVE_HUMIDITY,
            Self::Setback => method::SETBACK_STATE,
            Self::Mode => method::MODE_SETTING,
            Self::Heat => method::HEAT_SETPOINT,
            Self::Cool => method::COOL_SETPOINT,
            Self::SlabTarget => method::SLAB_SETPOINT,
            Self::Fan => method::FAN_PERCENT,
            Self::HumidityMin => method::HUMIDITY_MIN,
            Self::HumidityMax => method::HUMIDITY_MAX,
            Self::Events => method::SETBACK_EVENTS,
            Self::DeviceTarget => method::SETPOINT_DEVICE,
        }
    }
    pub(super) fn mask(self) -> u16 {
        1 << self as u8
    }
    pub(super) fn width(self) -> usize {
        if self == Self::DeviceTarget {
            return 3;
        }
        if matches!(
            self,
            Self::Attributes
                | Self::Temperature
                | Self::Floor
                | Self::Heat
                | Self::Cool
                | Self::SlabTarget
                | Self::Fan
        ) {
            2
        } else {
            1
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Telemetry {
    air: u16,
    slab: u16,
    demand: u8,
    humidity: u8,
    events: u8,
    setback: u8,
    mode: u8,
    reported_mode: Option<u8>,
    air_mode_hint: u8,
    air_slots: [[u8; setback::SLOT_COUNT]; 2],
    targets: [[u8; 2]; 4],
    rh_targets: [u8; 2],
    device_target: [u8; 3], // selector and little-endian degH from the same status
    pub(super) attributes: u16,
    floor: Floor,
    pub(super) dirty: u16,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Floor {
    #[default]
    Unknown,
    Absent,
    Mixed,
    Only,
}

fn normalized_temperature(word: [u8; 2]) -> u16 {
    match u16::from_le_bytes(word) {
        v if TEMP_FAULT_WORDS.contains(&v) => u16::MAX,
        other => other,
    }
}

impl Telemetry {
    const HEAT: u16 = 1;
    const COOL: u16 = 2;
    const FLOOR_ONLY: u16 = 4;
    const FAN: u16 = 8;
    const RH_SOURCE: u16 = 16;
    const RH_MAX: u16 = 32;
    const RH_MIN: u16 = 64;
    const SETPOINT_DEVICE: u16 = 128;
    const FLOOR_SENSOR: u16 = 256;

    pub(super) fn mark_setpoint_device(&mut self) {
        self.attributes |= Self::SETPOINT_DEVICE;
        self.dirty &= !(Field::Mode.mask() | Field::Heat.mask() | Field::Cool.mask());
    }
    pub(super) fn is_setpoint_device(&self) -> bool {
        self.attributes & Self::SETPOINT_DEVICE != 0
    }
    pub(super) fn mode(&self) -> Option<u8> {
        (self.mode != UNAVAILABLE).then_some(self.mode)
    }
    pub(super) fn setback(&self) -> u8 {
        self.setback
    }
    pub(super) const EMPTY: Self = Self {
        air: 0,
        slab: 0,
        demand: 0,
        humidity: 0,
        events: 0,
        setback: 0,
        mode: 0,
        reported_mode: None,
        air_mode_hint: 0,
        air_slots: [[0; setback::SLOT_COUNT]; 2],
        targets: [[0; 2]; 4],
        rh_targets: [0; 2],
        device_target: [0; 3],
        attributes: 0,
        floor: Floor::Unknown,
        dirty: 0,
    };
    pub(super) const UNKNOWN: Self = Self {
        air: u16::MAX,
        slab: u16::MAX,
        demand: UNAVAILABLE,
        humidity: UNAVAILABLE,
        setback: UNAVAILABLE,
        mode: UNAVAILABLE,
        air_slots: [[u8::MAX; setback::SLOT_COUNT]; 2],
        targets: [[UNAVAILABLE; 2]; 4],
        rh_targets: [UNAVAILABLE; 2],
        device_target: [UNAVAILABLE; 3],
        ..Self::EMPTY
    };
    pub(super) fn sensor_setup(&mut self, mask: u8, values: &[u8]) -> Option<u16> {
        if mask & SENSOR_MASK == 0 {
            return None;
        }
        let mut floors = 0;
        let mut off = 0;
        for i in 0..SENSOR_SLOTS {
            let value = if mask & (1 << i) == 0 {
                0
            } else {
                block_byte(mask, values, i)?
            };
            match value {
                SENSOR_ROLE_OFF => off += 1,
                SENSOR_ROLE_FLOOR => floors += 1,
                _ => {}
            }
        }
        let next = if floors == 0 {
            Floor::Absent
        } else if floors == 1 && off == SENSOR_SLOTS - 1 {
            Floor::Only
        } else {
            Floor::Mixed
        };
        let changed = self.floor != next;
        self.attributes &= !(Self::FLOOR_ONLY | Self::FLOOR_SENSOR);
        self.floor = next;
        if self.floor == Floor::Only {
            self.attributes &= !(Self::HEAT | Self::COOL);
            self.attributes |= Self::FLOOR_ONLY;
            self.dirty &= !(Field::Heat.mask() | Field::Cool.mask());
        }
        if !self.configured(Setting::Slab) {
            self.dirty &= !Field::SlabTarget.mask();
        } else {
            self.attributes |= Self::FLOOR_SENSOR;
        }
        Some(if changed {
            Field::Temperature.mask() | Field::Floor.mask()
        } else {
            0
        })
    }

    pub(super) fn configured(&self, setting: Setting) -> bool {
        match setting {
            Setting::DeviceTarget => self.is_setpoint_device(),
            Setting::Mode => !self.is_setpoint_device(),
            Setting::Slab => matches!(self.floor, Floor::Only | Floor::Mixed),
            Setting::Heat | Setting::Cool => {
                !self.is_setpoint_device() && self.floor != Floor::Only
            }
            Setting::HumidityMin => self.attributes & Self::RH_MIN != 0,
            Setting::HumidityMax => self.attributes & Self::RH_MAX != 0,
            _ => true,
        }
    }
    pub(super) fn has_rh(&self) -> bool {
        self.attributes & Self::RH_SOURCE != 0
    }
    fn refreshable(&self, id: u8) -> bool {
        let attributes = match id {
            block::HEAT_SETPOINTS => Self::HEAT,
            block::COOL_SETPOINTS => Self::COOL,
            block::SLAB_SETPOINTS => Self::FLOOR_ONLY | Self::FLOOR_SENSOR,
            block::FAN_PERCENT => Self::FAN,
            _ => return false,
        };
        self.attributes & attributes != 0
    }
    /// Changes justify reading configured mode; demand alone cannot tell us the mode.
    pub(super) fn mode_hint(&self) -> (u8, u8, u8, [u8; 2]) {
        let active = slots(self.setback);
        (
            self.air_mode_hint,
            self.demand,
            self.setback,
            self.air_slots
                .map(|side| active.map_or(UNAVAILABLE, |(i, _)| side[i])),
        )
    }
    pub(super) fn forget_mode_report(&mut self) {
        self.reported_mode = None;
    }
    pub(super) fn confirm_mode(&mut self, mode: u8) -> u16 {
        self.mode = mode;
        if self.reported_mode == Some(mode) {
            self.dirty &= !Field::Mode.mask();
            0
        } else {
            Field::Mode.mask()
        }
    }
    pub(super) fn missing_target(&self, id: u8) -> bool {
        match id {
            block::HEAT_SETPOINTS | block::COOL_SETPOINTS => {
                let side = if id == block::HEAT_SETPOINTS {
                    HEAT_REPORT
                } else {
                    COOL_REPORT
                };
                self.configured(Setting::Heat)
                    && self.refreshable(id)
                    && slots(self.setback)
                        .is_none_or(|(i, _)| self.air_slots[side][i] == UNAVAILABLE)
            }
            block::SLAB_SETPOINTS | block::FAN_PERCENT => {
                let index = if id == block::SLAB_SETPOINTS {
                    SLAB_REPORT
                } else {
                    FAN_REPORT
                };
                let [selector, value] = self.targets[index];
                self.refreshable(id) && (selector != self.setback || value == UNAVAILABLE)
            }
            block::HUMIDIFY_TARGETS => {
                self.configured(Setting::HumidityMin) && self.rh_targets[0] == UNAVAILABLE
            }
            block::DEHUMIDIFY_TARGETS => {
                self.configured(Setting::HumidityMax) && self.rh_targets[1] == UNAVAILABLE
            }
            _ => false,
        }
    }
    pub(super) fn setting_report(&mut self, data: &[u8]) -> u16 {
        let [block, 0, mask, values @ ..] = data else {
            return 0;
        };
        match *block {
            block::HEAT_SETPOINTS
            | block::COOL_SETPOINTS
            | block::SLAB_SETPOINTS
            | block::FAN_PERCENT => {
                if *block == block::FAN_PERCENT
                    && *mask & setback::ALL_SLOTS != 0
                    && block_byte(*mask, values, mask.trailing_zeros() as usize).is_some()
                {
                    self.attributes |= Self::FAN;
                }
                let (idx, field, setting) = match *block {
                    block::HEAT_SETPOINTS => (HEAT_REPORT, Field::Heat, Setting::Heat),
                    block::COOL_SETPOINTS => (COOL_REPORT, Field::Cool, Setting::Cool),
                    block::SLAB_SETPOINTS => (SLAB_REPORT, Field::SlabTarget, Setting::Slab),
                    _ => (FAN_REPORT, Field::Fan, Setting::Fan),
                };
                if !self.configured(setting) {
                    return 0;
                }
                if matches!(*block, block::HEAT_SETPOINTS | block::COOL_SETPOINTS) {
                    for (i, slot) in self.air_slots[idx].iter_mut().enumerate() {
                        if let Some(value) = block_byte(*mask, values, i) {
                            *slot = value;
                        }
                    }
                }
                let Some((i, _)) = slots(self.setback) else {
                    return 0;
                };
                let Some(value) = block_byte(*mask, values, i) else {
                    return 0;
                };
                self.targets[idx] = [self.setback, value];
                field.mask()
            }
            block::HUMIDITY_SETUP if self.has_rh() => {
                self.attributes &= !(Self::RH_MIN | Self::RH_MAX);
                for (i, (f, bit)) in [
                    (Field::HumidityMin, Self::RH_MIN),
                    (Field::HumidityMax, Self::RH_MAX),
                ]
                .into_iter()
                .enumerate()
                {
                    if block_byte(*mask, values, i).is_some_and(|v| v != 0 && v != UNAVAILABLE) {
                        self.attributes |= bit;
                    } else {
                        self.rh_targets[i] = UNAVAILABLE;
                        self.dirty &= !f.mask();
                    }
                }
                0
            }
            block::HUMIDIFY_TARGETS | block::DEHUMIDIFY_TARGETS => {
                let i = usize::from(*block - block::HUMIDIFY_TARGETS);
                if self.attributes & [Self::RH_MIN, Self::RH_MAX][i] == 0 {
                    return 0;
                }
                let Some(value) = block_byte(*mask, values, 0) else {
                    return 0;
                };
                self.rh_targets[i] = value;
                [Field::HumidityMin, Field::HumidityMax][i].mask()
            }
            _ => 0,
        }
    }

    pub(super) fn air_status(&mut self, air: AirStatus, setbacks: bool) -> u16 {
        self.air_mode_hint = air.mode_hint(self.air_mode_hint);
        self.air = normalized_temperature(air.temperature);
        self.demand = air.demand;
        self.setback = air.setback;
        let mut fields = Field::Temperature.mask() | Field::Demand.mask() | Field::Setback.mask();
        if self.floor == Floor::Only {
            return fields;
        }
        self.attributes |= Self::HEAT;
        if air.cool_capable {
            self.attributes |= Self::COOL;
        }
        if self.is_setpoint_device() {
            return fields;
        }
        let active = slots(self.setback);
        if let Some((side, value)) = air.target {
            let table = &mut self.air_slots[side as usize];
            if !setbacks {
                table.fill(value);
            } else if let Some((_, mask)) = active {
                for (i, slot) in table.iter_mut().enumerate() {
                    if mask & (1 << i) != 0 {
                        *slot = value;
                    }
                }
            }
        }
        for (side, field, attribute, excluded_mode) in [
            (Side::Heat, Field::Heat, Self::HEAT, mode::COOL),
            (Side::Cool, Field::Cool, Self::COOL, mode::HEAT),
        ] {
            if self.mode == mode::OFF
                || self.mode == excluded_mode
                || self.attributes & attribute == 0
            {
                self.dirty &= !field.mask();
                continue;
            }
            let i = side as usize;
            self.targets[i] = [
                self.setback,
                active.map_or(UNAVAILABLE, |(slot, _)| self.air_slots[i][slot]),
            ];
            fields |= field.mask();
        }
        fields
    }

    /// Called only after the enclosing dispatcher checks the native payload length.
    pub(super) fn observe(&mut self, message: u32, data: &[u8]) -> u16 {
        use Field::{Demand, Humidity, Setback, Temperature};
        match (message, data) {
            (
                message::SETPOINT_STATUS,
                &[state, operating, target_lo, target_hi, air_lo, air_hi, ..],
            ) if operating & setpoint::STATUS_MODE_MASK == setpoint::STATUS_RUNNING => {
                self.air = normalized_temperature([air_lo, air_hi]);
                self.demand = if operating & setpoint::STATUS_DEMAND == 0 {
                    demand::NONE
                } else if operating & setpoint::STATUS_COOL != 0 {
                    demand::COOL
                } else {
                    demand::HEAT
                };
                self.setback = (state >> 1) & setback::MASK;
                let mut fields =
                    Temperature.mask() | Demand.mask() | Setback.mask() | Field::Floor.mask();
                if self.setback < setback::UNKNOWN {
                    let [lo, hi] = normalized_temperature([target_lo, target_hi]).to_le_bytes();
                    self.device_target = [self.setback, lo, hi];
                    fields |= Field::DeviceTarget.mask();
                } else {
                    self.dirty &= !Field::DeviceTarget.mask();
                }
                fields
            }
            (message::RH_REPORT, &[_, _, control_rh, ..]) => {
                self.attributes |= Self::RH_SOURCE;
                self.humidity = control_rh;
                Humidity.mask()
            }
            (message::SCHEDULE_STATUS, &[flags, ..]) => {
                // Stock tHA copies bits 7:6, independently of mastership.
                self.events = flags >> EVENT_COUNT_SHIFT;
                Field::Events.mask()
                    | if self.setback < setback::UNKNOWN {
                        Setback.mask()
                    } else {
                        0
                    }
            }
            (message::FAN_DUCT_STATUS, _) => {
                self.attributes |= Self::FAN;
                0
            }
            (message::SLAB_STATUS, &[status, raw_target, lo, hi, ..]) => {
                const FIRST_NONPRIMARY_STATUS: u8 = 2;
                self.slab = normalized_temperature([lo, hi]);
                let target = if self.configured(Setting::Slab)
                    && self.setback < setback::UNKNOWN
                    && status < FIRST_NONPRIMARY_STATUS
                {
                    self.targets[SLAB_REPORT] = [self.setback, raw_target];
                    Field::SlabTarget.mask()
                } else {
                    0
                };
                target
                    | match self.floor {
                        Floor::Only => {
                            Field::Attributes.mask() | Temperature.mask() | Field::Floor.mask()
                        }
                        Floor::Mixed => Field::Attributes.mask() | Field::Floor.mask(),
                        _ => 0,
                    }
            }
            _ => 0,
        }
    }

    fn value(&self, field: Field) -> u16 {
        match field {
            Field::Attributes => self.attributes,
            Field::Temperature if self.floor == Floor::Only => self.slab,
            Field::Temperature => self.air,
            Field::Floor if matches!(self.floor, Floor::Only | Floor::Mixed) => self.slab,
            Field::Floor => u16::MAX,
            Field::Demand => self.demand.into(),
            Field::Humidity => self.humidity.into(),
            Field::Events => self.events.into(),
            Field::Setback => self.setback.into(),
            Field::Mode => self.mode.into(),
            Field::DeviceTarget => {
                u16::from_le_bytes([self.device_target[1], self.device_target[2]])
            }
            Field::Heat => u16::from_le_bytes(self.targets[HEAT_REPORT]),
            Field::Cool => u16::from_le_bytes(self.targets[COOL_REPORT]),
            Field::SlabTarget => u16::from_le_bytes(self.targets[SLAB_REPORT]),
            Field::Fan => u16::from_le_bytes(self.targets[FAN_REPORT]),
            Field::HumidityMin => self.rh_targets[0].into(),
            Field::HumidityMax => self.rh_targets[1].into(),
        }
    }
    fn body(&self, address: u16, field: Field) -> [u8; 5] {
        let [a, b] = address.to_le_bytes();
        let [c, d] = self.value(field).to_le_bytes();
        if field == Field::DeviceTarget {
            [a, b, self.device_target[0], c, d]
        } else {
            [a, b, c, d, 0]
        }
    }
    pub(super) fn reply(&self, address: u16, field: Field, call: &Call<'_>) -> Reply {
        call.ok(&self.body(address, field)[..2 + field.width()])
    }
    pub(super) fn report(&mut self, address: u16) -> Option<Reply> {
        let field = if self.dirty & Field::Mode.mask() != 0 {
            self.reported_mode = Some(self.mode);
            Field::Mode
        } else {
            *Field::ALL.get(self.dirty.trailing_zeros() as usize)?
        };
        self.dirty &= !field.mask();
        Some(Reply::report(
            field.method(),
            &self.body(address, field)[..2 + field.width()],
        ))
    }
}
