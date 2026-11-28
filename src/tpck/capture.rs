//! Stamped tN4 observations shared by Normal trace and Raw capture.
pub const NATIVE: u8 = 0;
pub const INVALID: u8 = 4;
pub const ACK: u8 = 1;
pub const EARLY_START: u8 = 1 << 1;
pub const PREFIX: usize = 2 + size_of::<u32>();
pub const INVALID_PREFIX: usize = PREFIX + 1 + size_of::<u16>();

pub fn prefix(event: u8, port: u8, ms: u32) -> [u8; PREFIX] {
    let [a, b, c, d] = ms.to_le_bytes();
    [event, port, a, b, c, d]
}

pub mod invalid {
    pub const STUFFING: u8 = 1;
    pub const CRC: u8 = 2;
    pub const CRC_DELIMITER: u8 = 3;
    pub const BAD_END: u8 = 4;
}
