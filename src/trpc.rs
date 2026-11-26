//! Normal-mode gateway identification and optional update management.
//! Device methods belong to the tN4 dispatcher, not to this layer.
use crate::tpck::{Packet, TRPC};

const UPDATE: u8 = 0;
const REQUEST: u8 = 1;
const RESPONSE_UPDATE: u8 = 3;
const RESPONSE_REQUEST: u8 = 4;
pub const BRIDGE_CAPABILITIES: u32 = 0x544b_f001;
pub const FIRMWARE_BUILD: u32 = 0x544b_f004;
pub const ENTER_FIRMWARE_UPDATE: u32 = 0x544b_f005;
const BRIDGE_ERROR: u32 = 0x544b_f006;

const BUILD_LEN: usize = env!("TEKMAR_BUILD_REVISION").len();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayVersion {
    Firmware = 0x187,
    Protocol = 0x18f,
}

impl GatewayVersion {
    pub(crate) fn from_method(method: u32) -> Option<Self> {
        match method {
            0x187 => Some(Self::Firmware),
            0x18f => Some(Self::Protocol),
            _ => None,
        }
    }

    pub(crate) fn word(self) -> [u8; 2] {
        match self {
            Self::Firmware => 0u16, // unnumbered custom firmware; FirmwareBuild identifies the image
            Self::Protocol => 3u16,
        }
        .to_le_bytes()
    }
}

// FirmwareBuild serves the installer's fixed-offset image stamp itself.
#[used]
#[unsafe(link_section = ".revision")]
static REVISION: [u8; 48] = {
    let mut field = [0; 48];
    let (header, text) = field.split_at_mut(8);
    header.copy_from_slice(b"TK48REV\0");
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
    bytes: [u8; 64],
    len: usize,
    pub after_drain: AfterDrain,
}

impl Reply {
    pub(crate) fn report(method: u32, body: &[u8]) -> Self {
        Self::new(2, method, body)
    }

    pub fn payload(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn new(service: u8, method: u32, body: &[u8]) -> Self {
        let mut reply = Self {
            bytes: [0; 64],
            len: body.len() + 5,
            after_drain: AfterDrain::Continue,
        };
        reply.bytes[0] = service;
        reply.bytes[1..5].copy_from_slice(&method.to_le_bytes());
        reply.bytes[5..reply.len].copy_from_slice(body);
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
        self.response(0, &[])
    }

    pub fn ok(&self, body: &[u8]) -> Reply {
        self.response(self.method, body)
    }

    pub fn error(&self, reason: u8) -> Reply {
        let mut body = [0; 5];
        body[..4].copy_from_slice(&self.method.to_le_bytes());
        body[4] = reason;
        self.response(BRIDGE_ERROR, &body)
    }

    pub fn management(&self, raw: bool, busy: bool) -> Option<Reply> {
        let reply = match (self.service, self.method) {
            (REQUEST, BRIDGE_CAPABILITIES) if self.body.is_empty() => {
                self.response(self.method, &[1, u8::from(raw), 5, 3])
            }
            (REQUEST, FIRMWARE_BUILD) if self.body.is_empty() => {
                self.response(self.method, &REVISION[8..8 + BUILD_LEN])
            }
            (UPDATE, ENTER_FIRMWARE_UPDATE) if self.body.is_empty() => {
                if busy {
                    self.error(1)
                } else {
                    let mut reply = self.response(self.method, &[]);
                    reply.after_drain = AfterDrain::EnterRom;
                    reply
                }
            }
            (_, BRIDGE_CAPABILITIES | FIRMWARE_BUILD | ENTER_FIRMWARE_UPDATE) => self.error(2),
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
