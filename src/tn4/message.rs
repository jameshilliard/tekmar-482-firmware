//! Native tN4 application message IDs used by the gateway.
pub const SETPOINT_ENABLE_REPORT: u32 = 0x20f;
pub const SETPOINT_ENABLE_UPDATE: u32 = 0x21f;
pub const SETPOINT_STATUS: u32 = 0x22f;
pub const FAN_DUCT_STATUS: u32 = 0x25f;
pub const SLAB_STATUS: u32 = 0x26f;
pub const BLOCK_REQUEST: u32 = 0x2d7;
pub const BLOCK_UPDATE: u32 = 0x2df;
pub const BLOCK_REPORT: u32 = 0x2e7;
pub const SCHEDULE_STATUS: u32 = 0x2f7;
pub const REAL_TIME_CLOCK_REPORT: u32 = 0x31f;
pub const RH_REPORT: u32 = 0x323;
pub const OUTDOOR_TEMPERATURE_REQUEST: u32 = 0x327;
pub const OUTDOOR_TEMPERATURE_REPORT: u32 = 0x32f;
pub const AIR_GROUP_MEMBER_STATUS: u32 = 0x36f;
pub const AIR_GROUP_MASTER_STATUS: u32 = 0x377;
pub const NETWORK_ERROR: u32 = 0x3d7;
pub const NETWORK_INIT_REQUEST: u32 = 0x3df;
pub const TAKING_ADDRESS: u32 = 0x3e7;
pub const MASTER_QUERY: u32 = 0x3f7;
pub const MASTER_PRESENT: u32 = 0x3ff;

/// Minimum payload sizes, excluding the native header and any extensions.
pub mod payload_bytes {
    pub const SETPOINT_STATUS: usize = 11;
    pub const FAN_DUCT_STATUS: usize = 5;
    pub const SLAB_STATUS: usize = 4;
    pub const BLOCK_REPORT_PREFIX: usize = size_of::<u16>() + 1;
    pub const SCHEDULE_STATUS: usize = 3;
    pub const RH_REPORT_PREFIX: usize = 6; // followed by the declared sensor bytes
    pub const AIR_GROUP_MEMBER_STATUS: usize = 8;
    pub const AIR_GROUP_MASTER_STATUS: usize = 13;
}
