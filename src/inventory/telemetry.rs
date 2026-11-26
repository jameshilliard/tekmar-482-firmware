//! Device readings common to the tHA Request, Response:Update and Report paths.
use super::block_byte;
use crate::trpc::{Call, Reply};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    Temperature,
    Floor,
    Demand,
    Humidity,
    Setback,
}
impl Field {
    const ALL: [Self; 5] = [
        Self::Temperature,
        Self::Floor,
        Self::Demand,
        Self::Humidity,
        Self::Setback,
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
        }
    }
    pub(super) fn mask(self) -> u8 {
        1 << self as u8
    }
    pub(super) fn width(self) -> usize {
        if matches!(self, Self::Temperature | Self::Floor) {
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
    setback: u8,
    floor: Floor,
    pub(super) dirty: u8,
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
    pub(super) const EMPTY: Self = Self {
        air: 0,
        slab: 0,
        demand: 0,
        humidity: 0,
        setback: 0,
        floor: Floor::Unknown,
        dirty: 0,
    };
    pub(super) const UNKNOWN: Self = Self {
        air: u16::MAX,
        slab: u16::MAX,
        demand: 255,
        humidity: 255,
        setback: 255,
        ..Self::EMPTY
    };
    pub(super) fn sensor_setup(&mut self, mask: u8, values: &[u8]) -> Option<u8> {
        let mut floors = 0;
        let mut off = 0;
        for i in 0..5 {
            let value = if mask & (1 << i) == 0 {
                0
            } else {
                block_byte(mask, values, i)?
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
        Some(if changed {
            Field::Temperature.mask() | Field::Floor.mask()
        } else {
            0
        })
    }

    /// Called only after the enclosing dispatcher checks the native payload length.
    pub(super) fn observe(&mut self, message: u32, data: &[u8]) -> u8 {
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
                self.humidity = data[2];
                return Humidity.mask();
            }
            0x26f => {
                self.slab = temperature(data, 2);
                return match self.floor {
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
            Field::Setback => self.setback.into(),
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
