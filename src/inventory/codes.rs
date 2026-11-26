//! Codes shared by Normal-mode application handlers.
pub(super) mod setback {
    pub const WAKE: u8 = 0;
    pub const UNOCCUPIED_FOUR_EVENT: u8 = 1;
    pub const OCCUPIED_FOUR_EVENT: u8 = 2;
    pub const SLEEP: u8 = 3;
    pub const OCCUPIED_TWO_EVENT: u8 = 4;
    pub const UNOCCUPIED_TWO_EVENT: u8 = 5;
    pub const AWAY: u8 = 6;
    pub const UNKNOWN: u8 = 7;
    pub const MASK: u8 = 7;
    pub const SLOT_COUNT: usize = 5;
    pub const ALL_SLOTS: u8 = (1 << SLOT_COUNT) - 1;
    const WAKE_SLOT: usize = 0;
    const UNOCCUPIED_SLOT: usize = 1;
    const OCCUPIED_SLOT: usize = 2;
    const SLEEP_SLOT: usize = 3;
    const AWAY_SLOT: usize = 4;

    #[derive(Clone, Copy)]
    pub enum Band {
        Occupied,
        Unoccupied,
        Away,
    }
    impl Band {
        pub fn from_selector(code: u8) -> Option<Self> {
            Some(match code {
                WAKE | OCCUPIED_FOUR_EVENT | OCCUPIED_TWO_EVENT => Self::Occupied,
                UNOCCUPIED_FOUR_EVENT | SLEEP | UNOCCUPIED_TWO_EVENT => Self::Unoccupied,
                AWAY => Self::Away,
                _ => return None,
            })
        }
        /// Thermostat blocks use five bytes, with occupied and unoccupied aliases.
        pub fn thermostat_slots(self) -> (usize, u8) {
            match self {
                Self::Occupied => (WAKE_SLOT, (1 << WAKE_SLOT) | (1 << OCCUPIED_SLOT)),
                Self::Unoccupied => (UNOCCUPIED_SLOT, (1 << UNOCCUPIED_SLOT) | (1 << SLEEP_SLOT)),
                Self::Away => (AWAY_SLOT, 1 << AWAY_SLOT),
            }
        }
        /// Setpoint controllers instead store three adjacent little-endian words.
        pub fn target_word_offset(self) -> usize {
            self as usize * size_of::<u16>()
        }
    }
}
pub(super) mod mode {
    pub const OFF: u8 = 0;
    pub const HEAT: u8 = 1;
    pub const COOL: u8 = 3;
    pub const VENT: u8 = 4;
    pub const EMERGENCY: u8 = 6;
}
pub(super) mod demand {
    pub const NONE: u8 = 0;
    pub const HEAT: u8 = 1;
    pub const COOL: u8 = 3;
}
pub(super) mod setpoint {
    pub const ON_OFF: u8 = 0;
    pub const HOT_TUB: u8 = 1;
    pub const DHW: u8 = 2;
    pub const PWM: u8 = 3;
    pub const FLOOR: u8 = 4;
    pub const POOL: u8 = 5;
    pub const STATUS_MODE_MASK: u8 = 0x30;
    pub const STATUS_RUNNING: u8 = 0x10;
    pub const STATUS_DEMAND: u8 = 1 << 2;
    pub const STATUS_COOL: u8 = 1 << 3;
}

pub const TEMP_FAULT_WORDS: [u16; 3] = [0x4000, 0x8000, 0xc000];
pub const UNAVAILABLE: u8 = u8::MAX;
