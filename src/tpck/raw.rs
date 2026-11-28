//! Service and outcome bytes in a TPCK tN4 bridge record.
pub const RECORD: u8 = 0;
pub const ALL_PORTS: u8 = 0x3f;
pub const EXTENSIONS: u8 = 1;
pub const TX_RESULT: u8 = 2;
pub const PORT_ERROR: u8 = 3;
pub const RX_INVALID: u8 = 4;
pub const RX_PREFIX: usize = 2 + size_of::<u32>();
pub const INVALID_PREFIX: usize = RX_PREFIX + 1 + size_of::<u16>();

pub fn send_body(data: &[u8]) -> Option<&[u8]> {
    (data.get(2..RX_PREFIX)? == [0; size_of::<u32>()]).then(|| &data[RX_PREFIX..])
}

pub mod port_error {
    pub const SEND_IN_PROGRESS: u8 = 1;
    pub const INVALID_PORT: u8 = 2;
}
pub mod status {
    pub const ACKNOWLEDGED: u8 = 0;
    pub const REJECTED: u8 = 1;
    pub const ATTEMPTED: u8 = 2;
}
pub mod rejected {
    pub const INVALID_BODY: u8 = 1;
    pub const PIN_UNAVAILABLE: u8 = 2;
    pub const LINK_RESPONSE_ACTIVE: u8 = 4;
}
pub mod attempted {
    pub const NO_ACK: u8 = 1;
    pub const ARBITRATION_LOST: u8 = 2;
    pub const ECHO_MISMATCH: u8 = 3;
    pub const BAD_END: u8 = 4;
    pub const MISSED_TIMING: u8 = 5;
}
pub mod invalid {
    pub const STUFFING: u8 = 1;
    pub const CRC: u8 = 2;
    pub const CRC_DELIMITER: u8 = 3;
    pub const BAD_END: u8 = 4;
}
