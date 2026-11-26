//! Device readings common to the tHA Request, Response:Update and Report paths.
use super::{block_byte, settings::Setting};
use crate::trpc::{Call, Reply};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
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
}
impl Field {
    const ALL: [Self; 13] = [
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
    ];
    pub(super) fn from_method(method: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.method() == method)
    }
    fn method(self) -> u32 {
        match self {
            Self::Temperature => 0x137,
            Self::Floor => 0x138,
            Self::Demand => 0x12f,
            Self::Humidity => 0x150,
            Self::Setback => 0x177,
            Self::Mode => 0x127,
            Self::Heat => 0x13f,
            Self::Cool => 0x147,
            Self::SlabTarget => 0x14f,
            Self::Fan => 0x157,
            Self::HumidityMin => 0x152,
            Self::HumidityMax => 0x151,
            Self::Events => 0x17f,
        }
    }
    pub(super) fn mask(self) -> u16 {
        1 << self as u8
    }
    pub(super) fn width(self) -> usize {
        if matches!(
            self,
            Self::Temperature
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
    targets: [[u8; 2]; 4],
    rh_targets: [u8; 2],
    observed_sources: u8,
    rh_enabled: u8,
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

fn temperature(data: &[u8], i: usize) -> u16 {
    match u16::from_le_bytes([data[i], data[i + 1]]) {
        0x4000 | 0x8000 | 0xc000 => u16::MAX,
        other => other,
    }
}

impl Telemetry {
    const RH_SOURCE: u8 = 1;
    const SETPOINT_DEVICE: u8 = 2;

    pub(super) fn mark_setpoint_device(&mut self) {
        self.observed_sources |= Self::SETPOINT_DEVICE;
    }
    pub(super) fn is_setpoint_device(&self) -> bool {
        self.observed_sources & Self::SETPOINT_DEVICE != 0
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
        targets: [[0; 2]; 4],
        rh_targets: [0; 2],
        observed_sources: 0,
        rh_enabled: 0,
        floor: Floor::Unknown,
        dirty: 0,
    };
    pub(super) const UNKNOWN: Self = Self {
        air: u16::MAX,
        slab: u16::MAX,
        demand: 255,
        humidity: 255,
        events: 255,
        setback: 255,
        mode: 255,
        targets: [[255; 2]; 4],
        rh_targets: [255; 2],
        ..Self::EMPTY
    };
    pub(super) fn sensor_setup(&mut self, mask: u8, values: &[u8]) -> Option<u16> {
        let mut floors = 0;
        let mut off = 0;
        for i in 0..5 {
            let value = if mask & (1 << i) == 0 {
                0
            } else {
                let Some(value) = block_byte(mask, values, i) else {
                    self.floor = Floor::Unknown;
                    self.dirty &= !Field::SlabTarget.mask();
                    return None;
                };
                value
            };
            match value {
                0 => off += 1,
                2 => floors += 1,
                _ => {}
            }
        }
        let next = if floors == 0 {
            Floor::Absent
        } else if floors == 1 && off == 4 {
            Floor::Only
        } else {
            Floor::Mixed
        };
        let changed = self.floor != next;
        self.floor = next;
        if self.floor == Floor::Only {
            self.dirty &= !(Field::Heat.mask() | Field::Cool.mask());
        }
        if !self.configured(Setting::Slab) {
            self.dirty &= !Field::SlabTarget.mask();
        }
        Some(if changed {
            Field::Temperature.mask() | Field::Floor.mask()
        } else {
            0
        })
    }

    pub(super) fn configured(&self, setting: Setting) -> bool {
        match setting {
            Setting::Slab => matches!(self.floor, Floor::Only | Floor::Mixed),
            Setting::Heat | Setting::Cool => self.floor != Floor::Only,
            Setting::HumidityMin => self.has_rh() && self.rh_enabled & 1 != 0,
            Setting::HumidityMax => self.has_rh() && self.rh_enabled & 2 != 0,
            _ => true,
        }
    }
    pub(super) fn has_rh(&self) -> bool {
        self.observed_sources & Self::RH_SOURCE != 0
    }
    pub(super) fn setting_report(&mut self, data: &[u8]) -> u16 {
        let [block, 0, mask, values @ ..] = data else {
            return 0;
        };
        match *block {
            3 => {
                let Some(mode) = block_byte(*mask, values, 0) else {
                    return 0;
                };
                self.mode = mode;
                Field::Mode.mask()
            }
            5 | 6 | 7 | 9 => {
                let (idx, field, setting) = match *block {
                    5 => (0, Field::Heat, Setting::Heat),
                    6 => (1, Field::Cool, Setting::Cool),
                    7 => (2, Field::SlabTarget, Setting::Slab),
                    _ => (3, Field::Fan, Setting::Fan),
                };
                if !self.configured(setting) {
                    return 0;
                }
                let Some((i, _)) = super::settings::slots(self.setback) else {
                    return 0;
                };
                let Some(value) = block_byte(*mask, values, i) else {
                    return 0;
                };
                self.targets[idx] = [self.setback, value];
                field.mask()
            }
            0xa3 => {
                self.rh_enabled = 0;
                for (i, f) in [Field::HumidityMin, Field::HumidityMax]
                    .into_iter()
                    .enumerate()
                {
                    if block_byte(*mask, values, i).is_some_and(|v| v != 0 && v != 255) {
                        self.rh_enabled |= 1 << i;
                    } else {
                        self.dirty &= !f.mask();
                    }
                }
                0
            }
            0xa4 | 0xa5 => {
                let i = usize::from(*block - 0xa4);
                if !self.has_rh() || self.rh_enabled & (1 << i) == 0 {
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

    /// Called only after the enclosing dispatcher checks the native payload length.
    pub(super) fn observe(&mut self, message: u32, data: &[u8]) -> u16 {
        use Field::{Demand, Humidity, Setback, Temperature};
        let (air, demand, setback) = match message {
            0x36f => (
                3,
                if data[1] & 3 != 3 {
                    0
                } else if data[0] & 32 == 0 {
                    1
                } else {
                    3
                },
                (data[1] >> 2) & 7,
            ),
            0x377 => (
                4,
                if data[2] & 3 == 3 {
                    1
                } else if data[6] & 3 == 3 {
                    3
                } else {
                    0
                },
                (data[2] >> 2) & 7,
            ),
            0x22f if data[1] & 0x30 == 0x10 => (
                4,
                if data[1] & 4 == 0 {
                    0
                } else if data[1] & 8 == 0 {
                    1
                } else {
                    3
                },
                (data[0] >> 1) & 7,
            ),
            0x323 => {
                self.observed_sources |= Self::RH_SOURCE;
                self.humidity = data[2];
                return Humidity.mask();
            }
            0x2f7 => {
                let code = data[0] >> 6;
                self.events = if data[0] & 4 != 0 && code < 3 {
                    code
                } else {
                    255
                };
                return Field::Events.mask() | if self.setback < 7 { Setback.mask() } else { 0 };
            }
            0x26f => {
                self.slab = temperature(data, 2);
                let target =
                    if self.configured(Setting::Slab) && self.setback < 7 && data[0] & 7 < 2 {
                        self.targets[2] = [self.setback, data[1]];
                        Field::SlabTarget.mask()
                    } else {
                        0
                    };
                return target
                    | match self.floor {
                        Floor::Only => Temperature.mask() | Field::Floor.mask(),
                        Floor::Mixed => Field::Floor.mask(),
                        _ => 0,
                    };
            }
            _ => return 0,
        };
        self.air = temperature(data, air);
        self.demand = demand;
        self.setback = setback;
        Temperature.mask() | Demand.mask() | Setback.mask()
    }

    fn value(&self, field: Field) -> u16 {
        match field {
            Field::Temperature if self.floor == Floor::Only => self.slab,
            Field::Temperature => self.air,
            Field::Floor if matches!(self.floor, Floor::Only | Floor::Mixed) => self.slab,
            Field::Floor => u16::MAX,
            Field::Demand => self.demand.into(),
            Field::Humidity => self.humidity.into(),
            Field::Events => self.events.into(),
            Field::Setback => self.setback.into(),
            Field::Mode => self.mode.into(),
            Field::Heat => u16::from_le_bytes(self.targets[0]),
            Field::Cool => u16::from_le_bytes(self.targets[1]),
            Field::SlabTarget => u16::from_le_bytes(self.targets[2]),
            Field::Fan => u16::from_le_bytes(self.targets[3]),
            Field::HumidityMin => self.rh_targets[0].into(),
            Field::HumidityMax => self.rh_targets[1].into(),
        }
    }
    fn body(&self, address: u16, field: Field) -> [u8; 4] {
        let [a, b] = address.to_le_bytes();
        let [c, d] = self.value(field).to_le_bytes();
        [a, b, c, d]
    }
    pub(super) fn reply(&self, address: u16, field: Field, call: &Call<'_>) -> Reply {
        call.ok(&self.body(address, field)[..2 + field.width()])
    }
    pub(super) fn report(&mut self, address: u16) -> Option<Reply> {
        let field = *Field::ALL.get(self.dirty.trailing_zeros() as usize)?;
        self.dirty &= !field.mask();
        Some(Reply::report(
            field.method(),
            &self.body(address, field)[..2 + field.width()],
        ))
    }
}
