//! Normal-mode trace metadata. Timestamps are gateway uptime at observation.
pub const RX: u8 = 0;
pub const TX: u8 = 1;
pub const INVALID: u8 = 4;
pub const ACK: u8 = 1;
pub const EARLY_START: u8 = 1 << 1;
pub const SETTING: u8 = 0;
pub const BROADCAST: u8 = 1;
pub const DISCOVERY: u8 = 2;
pub const PROBE: u8 = 3;
