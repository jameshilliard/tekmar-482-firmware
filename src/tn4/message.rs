//! Native tN4 application message IDs used by the gateway.
pub const ZONE_SYNC_ADJUSTMENT: u32 = 0x010;
pub const SETPOINT_ENABLE_REPORT: u32 = 0x20f;
pub const SETPOINT_ENABLE_UPDATE: u32 = 0x21f;
pub const SETPOINT_STATUS: u32 = 0x22f;
pub const FAN_DUCT_STATUS: u32 = 0x25f;
pub const SLAB_STATUS: u32 = 0x26f;
pub const BLOCK_REQUEST: u32 = 0x2d7;
pub const BLOCK_UPDATE: u32 = 0x2df;
pub const BLOCK_REPORT: u32 = 0x2e7;
pub const SCHEDULE_STATUS: u32 = 0x2f7;
pub const SCENE_REPORT: u32 = 0x2ff;
pub const REAL_TIME_CLOCK_REPORT: u32 = 0x31f;
pub const RH_REPORT: u32 = 0x323;
pub const OUTDOOR_TEMPERATURE_REQUEST: u32 = 0x327;
pub const OUTDOOR_TEMPERATURE_REPORT: u32 = 0x32f;
pub const STAGE_HEAT_DEMAND_REQUEST: u32 = 0x35f;
pub const AIR_GROUP_MEMBER_STATUS: u32 = 0x36f;
pub const AIR_GROUP_MASTER_STATUS: u32 = 0x377;
pub const WATER_CYCLE_START: u32 = 0x37f;
pub const END_PURGE: u32 = 0x397;
pub const START_PURGE: u32 = 0x39f;
pub const START_OVERRIDE_ON: u32 = 0x3a7;
pub const END_OVERRIDE_OFF: u32 = 0x3af;
pub const END_OVERRIDE_ON: u32 = 0x3b7;
pub const START_OVERRIDE_OFF: u32 = 0x3bf;
pub const NETWORK_ERROR: u32 = 0x3d7;
pub const NETWORK_INIT_REQUEST: u32 = 0x3df;
pub const TAKING_ADDRESS: u32 = 0x3e7;
pub const MASTER_QUERY: u32 = 0x3f7;
pub const MASTER_PRESENT: u32 = 0x3ff;

pub const fn is_stage_command(id: u32) -> bool {
    matches!(
        id,
        STAGE_HEAT_DEMAND_REQUEST
            | END_PURGE
            | START_PURGE
            | START_OVERRIDE_ON
            | END_OVERRIDE_OFF
            | END_OVERRIDE_ON
            | START_OVERRIDE_OFF
    )
}

/// A provable, single native destination, not the sender in the frame header.
pub fn single_destination(id: u32, payload: &[u8]) -> Option<[u8; 2]> {
    let (subnet, mask) = match (id, payload) {
        (BLOCK_UPDATE, [subnet, node, _, _, ..]) => {
            let node = *node & super::NODE_MASK;
            return (node != 0).then_some([*subnet, node]);
        }
        (BLOCK_REQUEST, [_, _, subnet, a, b, c, d, ..]) => {
            const BLOCK_NODES: u32 = u32::MAX >> 1; // bit 31 is padding
            (*subnet, u32::from_le_bytes([*a, *b, *c, *d]) & BLOCK_NODES)
        }
        (id, [subnet, _, a, b, c, ..]) if is_stage_command(id) => {
            (*subnet, u32::from_le_bytes([*a, *b, *c, 0]))
        }
        _ => return None,
    };
    mask.is_power_of_two()
        .then(|| [subnet, mask.trailing_zeros() as u8 + 1])
}

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
    pub const STAGE_SELECTION: usize = 5; // subnet, stage/level, 24-node mask
}
