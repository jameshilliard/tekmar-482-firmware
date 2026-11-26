//! Low byte of the little-endian configuration block ID (high byte zero).
pub const DEVICE_INFO: u8 = 0x00;
pub const HEATING_SETUP: u8 = 0x03;
pub const HEAT_SETPOINTS: u8 = 0x05;
pub const COOL_SETPOINTS: u8 = 0x06;
pub const SLAB_SETPOINTS: u8 = 0x07;
pub const FAN_PERCENT: u8 = 0x09;
pub const SENSOR_SETUP: u8 = 0x0d;
pub const SETPOINT_SETUP: u8 = 0x26;
pub const SETPOINT_TARGETS: u8 = 0x28;
pub const HOT_TUB_TARGETS: u8 = 0x2a;
pub const DHW_TARGETS: u8 = 0x2c;
pub const FLOOR_TARGETS: u8 = 0x2e;
pub const POOL_TARGETS: u8 = 0x30;
pub const HUMIDITY_SETUP: u8 = 0xa3;
pub const HUMIDIFY_TARGETS: u8 = 0xa4;
pub const DEHUMIDIFY_TARGETS: u8 = 0xa5;
