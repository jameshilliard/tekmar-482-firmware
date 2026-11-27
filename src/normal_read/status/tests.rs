extern crate std;
use super::*;
use std::vec::Vec;

fn key(peer: Option<[u8; 2]>, id: u16, payload: &[u8]) -> Vec<u8> {
    let [s, n] = peer.unwrap_or([0, 0]);
    [
        [peer.is_some() as u8, s, n].as_slice(),
        &id.to_le_bytes(),
        payload,
    ]
    .concat()
}

fn wire(id: u16, subnet: u8, node: u8, payload: &[u8]) -> Vec<u8> {
    let header =
        u32::from(id) << 20 | u32::from(subnet) << 12 | u32::from(node) << 6 | payload.len() as u32;
    [header.to_be_bytes().as_slice(), payload].concat()
}

fn observe(r: &mut StatusRead, frame: &[u8]) -> bool {
    observe_at(r, frame, 123)
}

fn observe_at(r: &mut StatusRead, frame: &[u8], now: u32) -> bool {
    let mut accepted = false;
    assert!(r.observe(2, frame, now, |part| {
        assert_eq!(&part.payload()[..7], &[2, 16, 0xf0, 0x4b, 0x54, 2, 0]);
        assert_eq!(&part.payload()[7..11], &now.to_le_bytes());
        assert_eq!(&part.payload()[11..], frame);
        accepted = true;
        true
    }));
    accepted
}

#[test]
fn ten_read_only_exchanges_have_independent_wire_ids_shapes_and_link_outcomes() {
    let cases: &[(u16, &[u8], u16, &[u8])] = &[
        (0x011, &[], 0x010, &[3, 0, 1]),
        (0x200, &[1], 0x200, &[0x82]),
        (0x307, &[], 0x30f, &[2]),
        (0x317, &[], 0x31f, &[0, 5, 1, 0, 25]),
        (0x327, &[], 0x32f, &[2, 0]),
        (0x3f7, &[], 0x3ff, &[]),
        (0x322, &[7, 0, 1, 0, 0], 0x323, &[2, 0, 0, 0, 0, 1, 66, 77]),
        (0x1f4, &[3, 9, 5], 0x1f3, &[3, 9, 5, 1, 2, 3]),
        (
            0x1f6,
            &[3, 9],
            0x1f5,
            &[3, 9, 255, 1, 65, 66, 67, 68, 69, 70, 1, 2, 3],
        ),
        (0x17f, &[2, 2, 66, 66], 0x17e, &[10, 1, 66, 1, 2]),
    ];
    for &(req, args, rep, data) in cases {
        let mut r = StatusRead::parse(&key(Some([7, 0]), req, args), 0).unwrap();
        assert_eq!(r.frame(2), Request::new(2, &wire(req, 0, 0, args)));
        let reply = wire(rep, 7, 32, data); // address lock is not part of the node
        assert!(!observe(&mut r, &reply));
        r.sent(Outcome::ArbitrationLost, 1);
        assert!(r.queued());
        r.sent(Outcome::NoAck, 2);
        assert!(!observe(&mut r, &wire(rep, 7, 1, data)));
        assert!(!observe(&mut r, &wire(rep, 6, 0, data)));
        if req != 0x3f7 {
            assert!(!observe(&mut r, &wire(rep, 7, 0, &[])));
        }
        if req == 0x200 {
            assert!(!observe(&mut r, &wire(rep, 7, 0, &[1])));
        }
        assert!(observe(&mut r, &reply));
        assert_eq!(&r.take_result(2).unwrap().payload()[5..], &[2, 1, 1]);
        assert!(r.take_result(2).is_none());
    }
    for (id, args) in [
        (0x2df, &[0, 0, 0, 0][..]),
        (0x200, &[0]),
        (0x200, &[3]),
        (0x17f, &[1, 0]),
        (0x17f, &[97, 1, 0]),
        (0x17f, &[194, 1, 0]),
        (0x17f, &[3, 1, 0]),
        (0x17f, &[1, 2, 0]),
        (0x317, &[1]),
        (0x322, &[1, 0, 0, 0, 0]),
    ] {
        assert!(StatusRead::parse(&key(None, id, args), 0).is_none());
    }
    assert!(StatusRead::parse(&[0, 1, 0, 0xf7, 3], 0).is_none());
    assert!(StatusRead::parse(&[1, 0, 32, 0xf7, 3], 0).is_none());
    assert!(StatusRead::parse(&key(Some([2, 1]), 0x322, &[1, 0, 0, 0, 0]), 0).is_none());
}

#[test]
fn sparse_snow_keys_and_channel_counts_never_combine_across_sources() {
    let mut r = StatusRead::parse(&key(None, 0x17f, &[1, 3, 0, 200, 0]), 0).unwrap();
    r.sent(Outcome::Ack, 1);
    for data in [&[9, 1, 200][..], &[17, 1, 0, 0, 0], &[9, 1, 17, 0, 0]] {
        assert!(!observe(&mut r, &wire(0x17e, 1, 1, data)));
    }
    assert!(observe(&mut r, &wire(0x17e, 8, 4, &[9, 1, 200, 5, 6])));
    assert!(observe(&mut r, &wire(0x17e, 8, 4, &[9, 1, 200, 7, 8])));
    assert!(!observe(&mut r, &wire(0x17e, 8, 5, &[9, 1, 0, 0, 0])));
    assert!(r.take_result(2).is_none());
    assert!(observe(&mut r, &wire(0x17e, 8, 36, &[9, 1, 0, 2, 3, 0xca])));
    assert_eq!(&r.take_result(2).unwrap().payload()[5..], &[2, 1, 0]);

    let mut r = StatusRead::parse(&key(None, 0x1f6, &[6, 9]), 0).unwrap();
    r.sent(Outcome::Ack, 1);
    for (at, id, total) in [(123, 255, 2), (423, 255, 2), (913, 0, 3), (932, 1, 3)] {
        let report = [6, 9, id, total, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(observe_at(&mut r, &wire(0x1f5, 99, 1, &report), at));
        assert!(r.take_result(2).is_none());
    }
    r.expire(123 + WINDOW_MS);
    assert_eq!(&r.take_result(2).unwrap().payload()[5..], &[2, 2, 0]);
}

#[test]
fn delayed_io_discovery_gets_a_full_send_and_response_window() {
    let start = u32::MAX - WINDOW_MS + 1400;
    let mut r = StatusRead::parse(&key(Some([6, 9]), 0x1f6, &[6, 9]), start).unwrap();
    r.sent(Outcome::ArbitrationLost, start.wrapping_add(WINDOW_MS / 2));
    let sent = start.wrapping_add(WINDOW_MS - 1);
    r.expire(sent);
    assert!(r.queued());
    r.sent(Outcome::Ack, sent);
    for (channel, elapsed) in [1900, 3250, 5200, 7450, 9300, 11_240]
        .into_iter()
        .enumerate()
    {
        let now = sent.wrapping_add(elapsed);
        r.expire(now);
        let report = [6, 9, channel as u8, 6, 0, 0, 0, 0, 0, 0, 1, 1, 0];
        assert!(observe_at(&mut r, &wire(0x1f5, 6, 9, &report), now));
    }
    assert_eq!(&r.take_result(2).unwrap().payload()[5..], &[2, 1, 0]);
}

#[test]
fn duplicate_keys_and_foreign_frames_do_not_extend_the_reply_window() {
    let mut r = StatusRead::parse(&key(Some([6, 9]), 0x17f, &[1, 2, 9, 22]), 0).unwrap();
    r.sent(Outcome::Ack, 11);
    let peer = wire(0x17e, 6, 9, &[9, 1, 9, 0, 0]);
    let foreign = wire(0x17e, 6, 10, &[9, 1, 22, 0, 0]);
    let unrelated = wire(0x17e, 6, 9, &[9, 1, 21, 0, 0]);
    assert!(observe_at(&mut r, &peer, 910));
    assert!(observe_at(&mut r, &peer, 9900));
    assert!(!observe_at(&mut r, &foreign, 9950));
    assert!(!observe_at(&mut r, &unrelated, 9998));
    r.expire(910 + WINDOW_MS - 1);
    assert!(r.take_result(2).is_none());
    r.expire(910 + WINDOW_MS);
    assert_eq!(&r.take_result(2).unwrap().payload()[5..], &[2, 2, 0]);
}

#[test]
fn uart_pressure_queue_deadlines_and_tx_faults_are_terminal_not_successes() {
    let new = || StatusRead::parse(&key(None, 0x3f7, &[]), 0).unwrap();
    let mut r = new();
    r.sent(Outcome::Ack, 1);
    assert!(!r.observe(1, &wire(0x3ff, 1, 1, &[]), 9, |_| false));
    assert_eq!(&r.take_result(1).unwrap().payload()[5..], &[1, 3, 0]);
    for (outcome, code) in [
        (Outcome::EchoMismatch, 3),
        (Outcome::BadEnd, 4),
        (Outcome::Late, 5),
    ] {
        let mut r = new();
        r.sent(outcome, 1);
        assert!(!observe(&mut r, &wire(0x3ff, 0, 0, &[])));
        assert_eq!(&r.take_result(1).unwrap().payload()[5..], &[1, 4, code]);
    }
    let mut r = new();
    r.unavailable();
    assert_eq!(&r.take_result(1).unwrap().payload()[5..], &[1, 4, 0xfe]);
    let mut r = new();
    r.expire(WINDOW_MS);
    assert_eq!(&r.take_result(1).unwrap().payload()[5..], &[1, 2, 0xff]);
}

#[test]
fn maximum_ordered_key_list_and_maximum_reply_fit_the_control_channel() {
    let mut args = Vec::from([1, 61]);
    args.extend(0..61);
    let mut r = StatusRead::parse(&key(None, 0x17f, &args), 0).unwrap();
    assert_eq!(r.frame(4), Request::new(4, &wire(0x17f, 0, 0, &args)));
    r.sent(Outcome::Ack, 1);
    for start in [0, 20, 40, 60] {
        let count = (61 - start).min(20);
        let mut data = Vec::from([9, count]);
        for id in start..start + count {
            data.extend([id, 0xca, 0x35]);
        }
        data.push(0x7e); // unclaimed extension, 63 bytes for a full part
        assert!(observe(&mut r, &wire(0x17e, 0, 31, &data)));
    }
    assert_eq!(&r.take_result(2).unwrap().payload()[5..], &[2, 1, 0]);
    args.push(61);
    args[1] = 62;
    assert!(StatusRead::parse(&key(None, 0x17f, &args), 0).is_none());
}
