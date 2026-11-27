//! Normal operations that return the link outcome without collecting application replies.
use super::{Query, TX_NOT_SENT, TX_PIN_UNAVAILABLE, WINDOW_MS, outcome_byte};
use crate::tn4::{
    HEADER_BYTES, MESSAGE_MASK, MESSAGE_SHIFT, PAYLOAD_MASK, block,
    message::{self, payload_bytes::STAGE_SELECTION},
    tx::{Outcome, Request},
};
use crate::tpck::trace;
use crate::trpc::{Reply, method};

const MAX_CLOCK_BYTES: usize = 6; // five date/time fields and optional DST mode
const WEEKDAY_MASK: u8 = 7;
const HOUR_SHIFT: u8 = 3;
const DST_FLAG: u8 = 1 << 4;
const SERVICE_BODY_BYTES: usize = HEADER_BYTES + 3; // largest known service payload

enum Phase {
    Queued,
    Done(u8),
    Idle,
}

enum Body {
    Announcement(u32, heapless::Vec<u8, MAX_CLOCK_BYTES>),
    IdentityProbe(u8),
    Stage(u32, [u8; STAGE_SELECTION]),
    Service(heapless::Vec<u8, SERVICE_BODY_BYTES>),
}

pub struct Send {
    body: Body,
    start: u32,
    phase: Phase,
}

impl Send {
    pub(super) fn announcement(key: &[u8], start: u32) -> Option<Self> {
        let (id, payload) = key.split_first_chunk::<2>()?;
        let message = u32::from(u16::from_le_bytes(*id));
        match (message, payload) {
            (message::ZONE_SYNC_ADJUSTMENT, [0..=1, 0..=1, 0..=2])
            | (message::SETPOINT_ENABLE_UPDATE, [1..=12, 0..=1])
            | (message::SCENE_REPORT, [0..=7, _, _])
            | (message::NETWORK_INIT_REQUEST, []) => {}
            (message::REAL_TIME_CLOCK_REPORT, [0..=59, time, month, 0..=30, _, extra @ ..])
                if extra.len() <= 1
                    && time >> HOUR_SHIFT < 24
                    && time & WEEKDAY_MASK != 0
                    && month & !DST_FLAG < 12 => {}
            _ => return None,
        }
        Some(Self {
            body: Body::Announcement(message, heapless::Vec::from_slice(payload).ok()?),
            start,
            phase: Phase::Queued,
        })
    }

    pub(super) fn discovery(key: &[u8], start: u32) -> Option<Self> {
        let &[subnet] = key else { return None };
        Some(Self {
            body: Body::IdentityProbe(subnet),
            start,
            phase: Phase::Queued,
        })
    }

    pub(super) fn stage(key: &[u8], start: u32) -> Option<Self> {
        let (id, payload) = key.split_first_chunk::<2>()?;
        let id = u32::from(u16::from_le_bytes(*id));
        let &[subnet, level, a, b, c] = payload else {
            return None;
        };
        if !message::is_stage_command(id) || (a | b | c) == 0 {
            return None;
        }
        Some(Self {
            body: Body::Stage(id, [subnet, level, a, b, c]),
            start,
            phase: Phase::Queued,
        })
    }

    pub(super) fn service_frame(body: &[u8], start: u32) -> Option<Self> {
        let (header, payload) = body.split_first_chunk::<HEADER_BYTES>()?;
        let id = (u32::from_be_bytes(*header) >> MESSAGE_SHIFT) & MESSAGE_MASK;
        if usize::from(header[HEADER_BYTES - 1] & PAYLOAD_MASK) != payload.len()
            || !matches!(
                (id, payload),
                (message::CALIBRATION_MODE, [_])
                    | (
                        message::CYCLE_LENGTH_REQUEST | message::NETWORK_TEST_TIME,
                        [_, _]
                    )
                    | (
                        message::NETWORK_TEST_MODE | message::MIXING_MODULE_UPDATE,
                        [_, _, _]
                    )
            )
        {
            return None;
        }
        Some(Self {
            body: Body::Service(heapless::Vec::from_slice(body).ok()?),
            start,
            phase: Phase::Queued,
        })
    }

    pub(super) fn trace_origin(&self) -> u8 {
        match self.body {
            Body::Announcement(..) => trace::BROADCAST,
            Body::IdentityProbe(_) => trace::DISCOVERY,
            Body::Stage(..) => trace::STAGE_COMMAND,
            Body::Service(_) => trace::SERVICE_FRAME,
        }
    }

    pub(super) fn queued(&self) -> bool {
        matches!(self.phase, Phase::Queued)
    }
    pub(super) fn frame(&self, port: u8) -> Option<Request> {
        if !self.queued() {
            return None;
        }
        match &self.body {
            Body::Announcement(message::NETWORK_INIT_REQUEST, data) => {
                Request::gateway(port, message::NETWORK_INIT_REQUEST, data)
            }
            Body::Announcement(id, data) => Request::gateway_class_zero(port, *id, data),
            Body::Stage(id, data) => Request::gateway_class_zero(port, *id, data),
            Body::Service(body) => Request::new(port, body),
            Body::IdentityProbe(subnet) => Query {
                nodes: super::ALL_NODES,
                subnet: *subnet,
                block: [block::DEVICE_INFO, 0],
                selector_mask: 0,
                selector: 0,
            }
            .frame(port),
        }
    }
    pub(super) fn sent(&mut self, outcome: Outcome) {
        self.finish(outcome_byte(outcome));
    }
    pub(super) fn unavailable(&mut self) {
        self.finish(TX_PIN_UNAVAILABLE);
    }
    pub(super) fn expire(&mut self, now: u32) {
        if now.wrapping_sub(self.start) >= WINDOW_MS {
            self.finish(TX_NOT_SENT);
        }
    }
    fn finish(&mut self, result: u8) {
        if self.queued() {
            self.phase = Phase::Done(result);
        }
    }
    pub(super) fn take_result(&mut self, port: u8) -> Option<Reply> {
        let Phase::Done(result) = self.phase else {
            return None;
        };
        self.phase = Phase::Idle;
        let method = match self.body {
            Body::Announcement(..) => method::NORMAL_ANNOUNCE,
            Body::IdentityProbe(_) => method::NORMAL_DISCOVER,
            Body::Stage(..) => method::NORMAL_STAGE_COMMAND,
            Body::Service(_) => method::NORMAL_SERVICE_FRAME,
        };
        Some(Reply::report(method, &[port, result]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_gateway_announcements_use_the_documented_wire_bodies() {
        for (key, body) in [
            (&[0x10, 0, 1, 0, 2][..], &[0x01, 0, 0, 3, 1, 0, 2][..]),
            (&[0x1f, 2, 12, 1], &[0x21, 0xf0, 0, 2, 12, 1]),
            (&[0xff, 2, 7, 0xe0, 1], &[0x2f, 0xf0, 0, 3, 7, 0xe0, 1]),
            (
                &[0x1f, 3, 30, 0xbf, 0x10, 6, 7],
                &[0x31, 0xf0, 0, 5, 30, 0xbf, 0x10, 6, 7],
            ),
            (
                &[0x1f, 3, 0, 1, 0, 0, 0, 0x85],
                &[0x31, 0xf0, 0, 6, 0, 1, 0, 0, 0, 0x85],
            ),
            (&[0xdf, 3], &[0x7d, 0xf0, 0, 0]),
        ] {
            let a = Send::announcement(key, 0).unwrap();
            assert_eq!(a.frame(3).unwrap(), Request::new(3, body).unwrap());
        }
        for bad in [
            &[][..],
            &[0xd7, 2, 0, 0],
            &[0x1f, 2, 0, 1],
            &[0x1f, 2, 13, 0],
            &[0x1f, 2, 2, 2],
            &[0x10, 0, 1, 0, 3],
            &[0x10, 0, 2, 0, 0],
            &[0xff, 2, 8, 1, 0],
            &[0xdf, 3, 0],
            &[0x1f, 3, 60, 1, 0, 0, 0],
            &[0x1f, 3, 0, 0, 0, 0, 0],
            &[0x1f, 3, 0, 0xc1, 0, 0, 0],
            &[0x1f, 3, 0, 1, 0x20, 0, 0],
            &[0x1f, 3, 0, 1, 12, 0, 0],
            &[0x1f, 3, 0, 1, 0, 31, 0],
            &[0x1f, 3, 0, 1, 0, 0, 0, 0, 0],
        ] {
            assert!(Send::announcement(bad, 0).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn stage_commands_admit_only_the_seven_exact_nonempty_wire_selectors() {
        for (key, header) in [
            ([0x5f, 3], [0x35, 0xf0, 0, 5]),
            ([0x97, 3], [0x39, 0x70, 0, 5]),
            ([0x9f, 3], [0x39, 0xf0, 0, 5]),
            ([0xa7, 3], [0x3a, 0x70, 0, 5]),
            ([0xaf, 3], [0x3a, 0xf0, 0, 5]),
            ([0xb7, 3], [0x3b, 0x70, 0, 5]),
            ([0xbf, 3], [0x3b, 0xf0, 0, 5]),
        ] {
            let payload = [0xab, 0xf5, 1, 0x80, 0x80];
            let mut a = Send::stage(&[key.as_slice(), &payload].concat(), 0).unwrap();
            let body = [header.as_slice(), &payload].concat();
            assert_eq!(a.frame(1), Request::new(1, &body));
            assert_eq!(a.trace_origin(), trace::STAGE_COMMAND);
            a.sent(Outcome::ArbitrationLost);
            a.sent(Outcome::Ack);
            assert!(a.frame(1).is_none());
            assert_eq!(
                a.take_result(1).unwrap().payload(),
                &[2, 0x15, 0xf0, 0x4b, 0x54, 1, 2]
            );
            assert!(a.take_result(1).is_none());
        }
        for bad in [
            &[0x9f][..],
            &[0x9f, 3, 0xab, 0xf5, 1, 0x80],
            &[0x9f, 3, 0xab, 0xf5, 1, 0x80, 0x80, 0],
            &[0x9f, 3, 0xab, 0xf5, 0, 0, 0],
            &[0x67, 3, 0xab, 0xf5, 1, 0x80, 0x80], // a StageHeatDemand report
            &[0xff, 2, 0xab, 0xf5, 1, 0x80, 0x80], // a scene
        ] {
            assert!(Send::stage(bad, 0).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn service_frames_preserve_the_header_and_admit_only_known_lengths() {
        for body in [
            &[0x68, 0x74, 0x31, 0xc1, 3][..],
            &[0xb3, 0xf4, 0x31, 0xc2, 3, 0x20],
            &[0xb5, 0x74, 0x31, 0xc3, 0xcf, 0xfe, 99],
            &[0xb8, 0x74, 0x31, 0xc2, 0x34, 0x12],
            &[0xb8, 0xf4, 0x31, 0xc3, 2, 0x56, 0x34],
        ] {
            let mut send = Send::service_frame(body, 0).unwrap();
            assert_eq!(send.frame(3), Request::new(3, body));
            assert_eq!(send.trace_origin(), trace::SERVICE_FRAME);
            send.sent(Outcome::ArbitrationLost);
            send.sent(Outcome::Ack);
            assert!(send.frame(3).is_none());
            assert_eq!(
                send.take_result(3).unwrap().payload(),
                &[2, 0x16, 0xf0, 0x4b, 0x54, 3, 2]
            );
            assert!(send.take_result(3).is_none());
        }
        for bad in [
            &[][..],
            &[0xb8, 0xf4, 0x31],
            &[0xb8, 0xf4, 0x31, 0xc2, 2, 0x56, 0x34], // length disagrees with header
            &[0xb8, 0xf4, 0x31, 0xc2, 2, 0x56],       // incomplete test mode
            &[0xb3, 0xf4, 0x31, 0xc1, 3],             // incomplete cycle length
            &[0xb3, 0xf4, 0x31, 0xc3, 3, 0x20, 0],    // unknown cycle length tail
            &[0xb5, 0x74, 0x31, 0xc4, 0xcf, 0xfe, 99, 0x77], // unknown tail
            &[0x2d, 0xf0, 0, 5, 0x43, 7, 3, 0, 1],    // unguarded BlockUpdate
        ] {
            assert!(Send::service_frame(bad, 0).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn one_terminal_per_port_even_for_uncertain_delivery_or_expiry() {
        let start = u32::MAX - 5;
        for (outcome, byte) in [
            (Some(Outcome::Ack), 0),
            (Some(Outcome::NoAck), 1),
            (Some(Outcome::ArbitrationLost), 2),
            (Some(Outcome::EchoMismatch), 3),
            (Some(Outcome::BadEnd), 4),
            (Some(Outcome::Late), 5),
            (None, 0xff),
            (None, 0xfe),
        ] {
            let mut a = Send::announcement(&[0x1f, 2, 1, 1], start).unwrap();
            a.expire(start.wrapping_add(WINDOW_MS - 1));
            assert!(a.queued() && a.take_result(4).is_none());
            if let Some(out) = outcome {
                a.sent(out);
            } else if byte == 0xfe {
                a.unavailable();
            }
            a.expire(start.wrapping_add(WINDOW_MS));
            a.sent(Outcome::Ack);
            assert!(a.frame(4).is_none());
            assert_eq!(
                a.take_result(4).unwrap().payload(),
                &[2, 0x12, 0xf0, 0x4b, 0x54, 4, byte]
            );
            assert!(!a.queued() && a.take_result(4).is_none());
        }
    }
}
