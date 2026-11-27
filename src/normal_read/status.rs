//! Read-only 483 native status exchanges. A frame is forwarded in full, not rebuilt.
use super::{TX_NOT_SENT, TX_PIN_UNAVAILABLE, WINDOW_MS, outcome_byte};
use crate::tn4::{
    self, HEADER_BYTES, NODE_MASK, PAYLOAD_MASK, snowmelt,
    tx::{Outcome, Request},
};
use crate::trpc::{Reply, method};

#[cfg(test)]
mod tests;

const ANY_PEER: u8 = 0;
const EXACT_PEER: u8 = 1;
const ACCESS_QUERY: u8 = 1;
const IO_DISCOVERY_BYTES: usize = 13;
const IO_VALUE_BYTES: usize = 6;
const HUMIDITY_PREFIX: usize = tn4::message::payload_bytes::RH_REPORT_PREFIX;
const FRAME_EVENT: u8 = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
enum Kind {
    ZoneSync = 0x011,
    Snow = snowmelt::REQUEST as u16,
    IoChannel = 0x1f4,
    IoDiscovery = 0x1f6,
    Access = 0x200,
    Wwsd = 0x307,
    Clock = 0x317,
    Humidity = 0x322,
    Outdoor = 0x327,
    Master = 0x3f7,
}

impl Kind {
    fn from_request(id: u16, args: &[u8]) -> Option<Self> {
        use Kind::*;
        [
            ZoneSync,
            Snow,
            IoChannel,
            IoDiscovery,
            Access,
            Wwsd,
            Clock,
            Humidity,
            Outdoor,
            Master,
        ]
        .into_iter()
        .find(|&k| {
            k as u16 == id
                && match (k, args) {
                    (Snow, [addr, len, keys @ ..]) => {
                        !keys.is_empty()
                            && usize::from(*len) == keys.len()
                            && snowmelt::report_address(*addr).is_some()
                    }
                    (IoChannel, [_, _, _])
                    | (IoDiscovery, [_, _])
                    | (Humidity, [_, _, _, _, _])
                    | (Access, [ACCESS_QUERY])
                    | (ZoneSync | Wwsd | Clock | Outdoor | Master, []) => true,
                    _ => false,
                }
        })
    }
    fn report(self) -> u32 {
        match self {
            Self::ZoneSync => tn4::message::ZONE_SYNC_ADJUSTMENT,
            Self::Snow => snowmelt::REPORT,
            Self::IoChannel => 0x1f3,
            Self::IoDiscovery => 0x1f5,
            Self::Access => Self::Access as u32,
            Self::Wwsd => 0x30f,
            Self::Clock => tn4::message::REAL_TIME_CLOCK_REPORT,
            Self::Humidity => tn4::message::RH_REPORT,
            Self::Outdoor => tn4::message::OUTDOOR_TEMPERATURE_REPORT,
            Self::Master => tn4::message::MASTER_PRESENT,
        }
    }
}

#[derive(Default)]
struct Items([u8; 32]); // one bit for each native u8 snow key or I/O channel
impl Items {
    fn has(&self, key: u8) -> bool {
        self.0[usize::from(key / 8)] & (1 << (key % 8)) != 0
    }
    fn insert(&mut self, key: u8) -> bool {
        let new = !self.has(key);
        self.0[usize::from(key / 8)] |= 1 << (key % 8);
        new
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum End {
    Complete = 1,
    Expired = 2,
    ControlFull = 3,
    TransmitFault = 4,
}

#[derive(Clone, Copy)]
enum Phase {
    Queued,
    Listening,
    Done(End),
    Idle,
}

pub struct StatusRead {
    kind: Kind,
    args: heapless::Vec<u8, { PAYLOAD_MASK as usize }>,
    peer: Option<[u8; 2]>,
    wanted: Items,
    seen: Items,
    expected: Option<u8>,
    covered: u16, // up to 256 distinct channel IDs, even if a peer misreports its count
    stamp: u32,
    phase: Phase,
    tx: u8,
}

impl StatusRead {
    pub(super) fn parse(body: &[u8], now: u32) -> Option<Self> {
        let &[filter, subnet, node, lo, hi, ref payload @ ..] = body else {
            return None;
        };
        let args = heapless::Vec::from_slice(payload).ok()?;
        let kind = Kind::from_request(u16::from_le_bytes([lo, hi]), &args)?;
        let peer = match (filter, subnet, node) {
            (ANY_PEER, 0, 0) => None,
            (EXACT_PEER, s, n @ 0..=NODE_MASK) => Some([s, n]),
            _ => return None,
        };
        if kind == Kind::Humidity && peer.map(|p| p[0]) != Some(args[0]) {
            return None;
        }
        let mut wanted = Items::default();
        let expected = match kind {
            Kind::Snow => Some(args[2..].iter().filter(|&&k| wanted.insert(k)).count() as u8),
            Kind::IoDiscovery => None, // first matching peer declares the number of channels
            _ => Some(1),
        };
        Some(Self {
            kind,
            args,
            peer,
            wanted,
            seen: Items::default(),
            expected,
            covered: 0,
            stamp: now,
            phase: Phase::Queued,
            tx: TX_NOT_SENT,
        })
    }
    pub(super) fn queued(&self) -> bool {
        matches!(self.phase, Phase::Queued)
    }
    pub(super) fn frame(&self, port: u8) -> Option<Request> {
        self.queued()
            .then(|| Request::gateway_class_zero(port, self.kind as u32, &self.args))
            .flatten()
    }
    pub(super) fn sent(&mut self, outcome: Outcome, now: u32) {
        if !self.queued() || outcome == Outcome::ArbitrationLost {
            return;
        }
        self.tx = outcome_byte(outcome);
        self.phase = if matches!(outcome, Outcome::Ack | Outcome::NoAck) {
            self.stamp = now;
            Phase::Listening
        } else {
            Phase::Done(End::TransmitFault)
        };
    }
    pub(super) fn unavailable(&mut self) {
        self.tx = TX_PIN_UNAVAILABLE;
        self.phase = Phase::Done(End::TransmitFault);
    }
    pub(super) fn expire(&mut self, now: u32) {
        if matches!(self.phase, Phase::Queued | Phase::Listening)
            && now.wrapping_sub(self.stamp) >= WINDOW_MS
        {
            self.phase = Phase::Done(End::Expired);
        }
    }

    fn accepts(&mut self, data: &[u8]) -> bool {
        match self.kind {
            Kind::Snow => {
                let Some((address, pairs)) = snowmelt::status(data) else {
                    return false;
                };
                if Some(address) != snowmelt::report_address(self.args[0]) {
                    return false;
                }
                let mut matched = false;
                for pair in pairs {
                    if self.wanted.has(pair[0]) {
                        matched = true;
                        self.covered += u16::from(self.seen.insert(pair[0]));
                    }
                }
                return matched;
            }
            Kind::IoDiscovery => {
                if data.len() < IO_DISCOVERY_BYTES || !data.starts_with(&self.args) {
                    return false;
                }
                let count = data[3];
                if *self.expected.get_or_insert(count) != count {
                    self.expected = Some(0);
                }
                self.covered += u16::from(self.seen.insert(data[2]));
                return true;
            }
            Kind::IoChannel if data.len() < IO_VALUE_BYTES || !data.starts_with(&self.args) => {
                return false;
            }
            Kind::Access if !data.first().is_some_and(|flags| flags & ACCESS_QUERY == 0) => {
                return false;
            }
            Kind::Humidity
                if data
                    .first()
                    .is_none_or(|n| data.len() < HUMIDITY_PREFIX + usize::from(*n)) =>
            {
                return false;
            }
            Kind::ZoneSync if data.len() < 3 => return false,
            Kind::Wwsd if data.is_empty() => return false,
            Kind::Clock if data.len() < 5 => return false,
            Kind::Outdoor if data.len() < 2 || data.len() == 3 => return false,
            _ => {}
        }
        self.covered = 1;
        true
    }

    /// False only when a matching frame could not enter the control FIFO.
    pub(super) fn observe(
        &mut self,
        port: u8,
        body: &[u8],
        now: u32,
        publish: impl FnOnce(&Reply) -> bool,
    ) -> bool {
        if !matches!(self.phase, Phase::Listening) {
            return true;
        }
        let Some((header, payload)) = body.split_first_chunk::<HEADER_BYTES>() else {
            return true;
        };
        let h = u32::from_be_bytes(*header);
        let peer = [
            (h >> tn4::SUBNET_SHIFT) as u8,
            (h >> tn4::NODE_SHIFT) as u8 & NODE_MASK,
        ];
        let covered = self.covered;
        if (h >> tn4::MESSAGE_SHIFT) & tn4::MESSAGE_MASK != self.kind.report()
            || usize::from(header[HEADER_BYTES - 1] & PAYLOAD_MASK) != payload.len()
            || self.peer.is_some_and(|p| p != peer)
            || !self.accepts(payload)
        {
            return true;
        }
        self.peer = Some(peer);
        let mut part = [0; super::MAX_RESULT];
        part[0] = port;
        part[1] = FRAME_EVENT;
        part[2..super::RESULT_PREFIX].copy_from_slice(&now.to_le_bytes());
        let end = super::RESULT_PREFIX + body.len();
        part[super::RESULT_PREFIX..end].copy_from_slice(body);
        if !publish(&Reply::report(method::NORMAL_STATUS_READ, &part[..end])) {
            self.phase = Phase::Done(End::ControlFull);
            return false;
        }
        if let Some(expected @ 1..=u8::MAX) = self.expected {
            if self.covered == u16::from(expected) {
                self.phase = Phase::Done(End::Complete);
            } else if self.covered > covered {
                // Only new coverage extends the window, not updates to an old item.
                self.stamp = now;
            }
        }
        true
    }
    pub(super) fn take_result(&mut self, port: u8) -> Option<Reply> {
        let Phase::Done(end) = self.phase else {
            return None;
        };
        self.phase = Phase::Idle;
        Some(Reply::report(
            method::NORMAL_STATUS_READ,
            &[port, end as u8, self.tx],
        ))
    }
}
