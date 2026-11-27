//! Normal-mode gateway identification and optional update management.
//! Device methods belong to the tN4 dispatcher, not to this layer.
use crate::tpck::{Packet, TRPC};
pub mod method;
use method::*;

pub mod service {
    pub const UPDATE: u8 = 0;
    pub const REQUEST: u8 = 1;
    pub const REPORT: u8 = 2;
    pub const RESPONSE_UPDATE: u8 = 3;
    pub const RESPONSE_REQUEST: u8 = 4;
}
use service::*;

#[repr(u8)]
pub enum BridgeError {
    Busy = 1,
    InvalidRequest = 2,
    WrongMode = 3,
}

const THA_VERSION: u16 = 3;
const UNNUMBERED_FIRMWARE: u16 = 0;
const BRIDGE_VERSION: u8 = 2;
const FEATURE_BIDIRECTIONAL_RAW: u8 = 1;
const FEATURE_ROM_UPDATE: u8 = 1 << 1;
const FEATURE_NORMAL_TRACE: u8 = 1 << 2;
const FEATURE_ERROR_REPORTING: u8 = 1 << 3;
const FEATURE_NORMAL_IDENTITY: u8 = 1 << 4;
const FEATURE_TRAFFIC_COUNTS: u8 = 1 << 5;
const FEATURE_NORMAL_BLOCK_READ: u8 = 1 << 6;
const HEADER_BYTES: usize = 1 + size_of::<u32>();
const REVISION_MAGIC: &[u8; 8] = b"TK48REV\0";
const REVISION_BYTES: usize = REVISION_MAGIC.len() + 40; // hexadecimal Git object name
const MAX_REPLY_BYTES: usize = HEADER_BYTES + crate::normal_read::MAX_RESULT;
const _: () = assert!(crate::normal_read::MAX_RESULT >= crate::diagnostics::CAPTURE_BYTES);

const BUILD_LEN: usize = env!("TEKMAR_BUILD_REVISION").len();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum GatewayVersion {
    Firmware = FIRMWARE_REVISION,
    Protocol = PROTOCOL_VERSION,
}

impl GatewayVersion {
    pub(crate) fn from_method(method: u32) -> Option<Self> {
        match method {
            FIRMWARE_REVISION => Some(Self::Firmware),
            PROTOCOL_VERSION => Some(Self::Protocol),
            _ => None,
        }
    }

    pub(crate) fn word(self) -> [u8; 2] {
        match self {
            Self::Firmware => UNNUMBERED_FIRMWARE,
            Self::Protocol => THA_VERSION,
        }
        .to_le_bytes()
    }
}

// FirmwareBuild serves the installer's fixed-offset image stamp itself.
#[used]
#[unsafe(link_section = ".revision")]
static REVISION: [u8; REVISION_BYTES] = {
    let mut field = [0; REVISION_BYTES];
    let (header, text) = field.split_at_mut(REVISION_MAGIC.len());
    header.copy_from_slice(REVISION_MAGIC);
    text.split_at_mut(BUILD_LEN)
        .0
        .copy_from_slice(env!("TEKMAR_BUILD_REVISION").as_bytes());
    field
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterDrain {
    Continue,
    EnterRom,
}

/// The caller must send the entire response and drain the UART before EnterRom.
pub struct Reply {
    bytes: [u8; MAX_REPLY_BYTES],
    len: usize,
    pub after_drain: AfterDrain,
}

impl Reply {
    pub fn report(method: u32, body: &[u8]) -> Self {
        Self::new(REPORT, method, body)
    }

    pub fn payload(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn new(service: u8, method: u32, body: &[u8]) -> Self {
        let mut reply = Self {
            bytes: [0; MAX_REPLY_BYTES],
            len: body.len() + HEADER_BYTES,
            after_drain: AfterDrain::Continue,
        };
        reply.bytes[0] = service;
        reply.bytes[1..HEADER_BYTES].copy_from_slice(&method.to_le_bytes());
        reply.bytes[HEADER_BYTES..reply.len].copy_from_slice(body);
        reply
    }
}

/// Unhandled device requests fall through so an application dispatcher may
/// answer them. Only after that dispatcher declines should it send NullMethod.
pub struct Call<'a> {
    pub service: u8,
    pub method: u32,
    pub body: &'a [u8],
}

impl<'a> Call<'a> {
    pub fn parse(packet: Packet<'a>) -> Option<Self> {
        let [service @ (UPDATE | REQUEST), a, b, c, d, body @ ..] = packet.data else {
            return None;
        };
        (packet.kind == TRPC).then(|| Self {
            service: *service,
            method: u32::from_le_bytes([*a, *b, *c, *d]),
            body,
        })
    }

    fn response(&self, method: u32, body: &[u8]) -> Reply {
        Reply::new(
            if self.service == REQUEST {
                RESPONSE_REQUEST
            } else {
                RESPONSE_UPDATE
            },
            method,
            body,
        )
    }

    pub fn unsupported(&self) -> Reply {
        self.response(NULL, &[])
    }

    pub fn ok(&self, body: &[u8]) -> Reply {
        self.response(self.method, body)
    }

    pub fn error(&self, reason: BridgeError) -> Reply {
        let mut body = [0; 5];
        body[..4].copy_from_slice(&self.method.to_le_bytes());
        body[4] = reason as u8;
        self.response(BRIDGE_ERROR, &body)
    }

    pub fn management(&self, raw: bool, busy: bool) -> Option<Reply> {
        let reply = match (self.service, self.method) {
            (REQUEST, BRIDGE_CAPABILITIES) if self.body.is_empty() => self.response(
                self.method,
                &[
                    BRIDGE_VERSION,
                    u8::from(raw),
                    crate::tn4::PORT_COUNT as u8,
                    FEATURE_BIDIRECTIONAL_RAW
                        | FEATURE_ROM_UPDATE
                        | FEATURE_NORMAL_TRACE
                        | FEATURE_ERROR_REPORTING
                        | FEATURE_NORMAL_IDENTITY
                        | FEATURE_TRAFFIC_COUNTS
                        | FEATURE_NORMAL_BLOCK_READ,
                ],
            ),
            (REQUEST, FIRMWARE_BUILD) if self.body.is_empty() => {
                self.response(self.method, &REVISION[REVISION_MAGIC.len()..][..BUILD_LEN])
            }
            (UPDATE, ENTER_FIRMWARE_UPDATE) if self.body.is_empty() => {
                if busy {
                    self.error(BridgeError::Busy)
                } else {
                    let mut reply = self.response(self.method, &[]);
                    reply.after_drain = AfterDrain::EnterRom;
                    reply
                }
            }
            (_, BRIDGE_CAPABILITIES | FIRMWARE_BUILD | ENTER_FIRMWARE_UPDATE) => {
                self.error(BridgeError::InvalidRequest)
            }
            _ => return None,
        };
        Some(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_idle_update_may_enter_rom() {
        let method = ENTER_FIRMWARE_UPDATE.to_le_bytes();
        for (svc, tail, busy, expected) in [
            (REQUEST, &[][..], false, AfterDrain::Continue),
            (UPDATE, &[0][..], false, AfterDrain::Continue),
            (UPDATE, &[], true, AfterDrain::Continue),
            (UPDATE, &[], false, AfterDrain::EnterRom),
        ] {
            let reply = Call {
                service: svc,
                method: ENTER_FIRMWARE_UPDATE,
                body: tail,
            }
            .management(false, busy)
            .unwrap();
            assert_eq!(reply.after_drain, expected);
            if expected == AfterDrain::Continue {
                assert_eq!(&reply.payload()[1..5], &BRIDGE_ERROR.to_le_bytes());
                assert_eq!(&reply.payload()[5..9], &method);
                assert_eq!(reply.payload()[9], if busy { 1 } else { 2 });
            } else {
                assert_eq!(reply.payload(), &[3, 5, 0xf0, 0x4b, 0x54]);
            }
        }
    }

    #[test]
    fn no_response_to_reports_short_headers_or_unrelated_packet_types() {
        for (kind, data) in [
            (TRPC, &[2, 0, 0, 0, 0][..]),
            (TRPC, &[3, 0, 0, 0, 0]),
            (TRPC, &[4, 0, 0, 0, 0]),
            (TRPC, &[1, 0, 0, 0]),
            (1, &[1, 0, 0, 0, 0]),
        ] {
            assert!(Call::parse(Packet { kind, data }).is_none());
        }
    }
}
