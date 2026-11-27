use super::*;
const KEY: [u8; 9] = [0x12, 1, 0, 0, 0, 0x38, 0, 7, 3];
const REPLY: [u8; 9] = [0x6e, 0x71, 0x28, 0x45, 0x38, 0, 3, 0xf3, 0xca];

#[test]
fn identity_discovery_is_one_read_only_link_attempt_not_a_bitmap_collector() {
    for key in [&[][..], &[1, 2]] {
        assert!(NormalOperation::parse(method::NORMAL_DISCOVER, key, 0).is_err());
    }
    for (outcome, code) in [
        (Outcome::Ack, 0),
        (Outcome::NoAck, 1),
        (Outcome::ArbitrationLost, 2),
    ] {
        let mut op = NormalOperation::parse(method::NORMAL_DISCOVER, &[0xff], 0).unwrap();
        assert!(op.single_attempt());
        assert_eq!(op.trace_origin(), crate::tpck::trace::DISCOVERY);
        assert_eq!(
            op.frame(4),
            Request::new(
                4,
                &[
                    0x6d, 0x70, 0, 11, 0, 0, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0
                ]
            )
        );
        op.observe(4, &REPLY, 1, |_| {
            panic!("probe cannot publish a peer identity")
        });
        assert!(op.take_result(4).is_none());
        op.sent(outcome, 2);
        assert_eq!(
            op.take_result(4).unwrap().payload(),
            &[2, 0x13, 0xf0, 0x4b, 0x54, 4, code]
        );
        assert!(op.frame(4).is_none() && op.take_result(4).is_none());
    }
}

#[test]
fn fresh_peer_only_after_transmit_with_selector_and_valid_layout() {
    let mut slot = BlockRead::EMPTY;
    let q = Query::parse(&KEY).unwrap();
    assert!(slot.admit(q, 50));
    assert!(!slot.admit(q, 52));
    assert_eq!(
        slot.frame(2),
        Request::new(
            2,
            &[0x6d, 0x70, 0, 11, 0x38, 0, 0x12, 1, 0, 0, 0, 3, 0, 0, 0]
        )
    );
    slot.sent(Outcome::ArbitrationLost, 51);
    slot.observe(2, &REPLY, 51, |_| panic!());
    assert!(slot.take_result(2).is_none() && slot.queued());
    slot.sent(Outcome::NoAck, 53);
    for (i, byte) in [
        (0, 0x6f),
        (1, 0x72),
        (2, 0x29),
        (3, 0x46),
        (4, 0x37),
        (6, 2),
        (6, 0x81),
        (6, 0xff),
        (7, 0xf4),
    ] {
        let mut other = REPLY;
        other[i] = byte;
        // With presence bits 0 and 7, two data bytes are a valid compact layout.
        if i == 6 && byte == 0x81 {
            other[7] = 1;
        }
        slot.observe(2, &other, 54, |_| panic!());
        assert!(slot.take_result(2).is_none());
    }
    assert!(
        q.matching_bit(&[0x6e, 0x71, 0x28, 0x46, 0x38, 0, 0x81, 3, 4, 5])
            .is_none()
    );
    slot.observe(2, &REPLY, 56, |_| panic!());
    assert!(!slot.admit(q, 57));
    let result = slot.take_result(2).unwrap();
    assert_eq!(
        &result.payload()[..12],
        &[2, 15, 0xf0, 0x4b, 0x54, 2, 0, 1, 56, 0, 0, 0]
    );
    assert_eq!(&result.payload()[12..], &REPLY);
    assert!(!slot.active() && slot.take_result(2).is_none());
}

#[test]
fn expiry_failures_and_maximum_extension_never_produce_cached_data() {
    let q = Query::parse(&[0x12, 1, 0, 0, 0, 0xcd, 0xab, 0, 0]).unwrap();
    for bad in [
        &KEY[..5],
        &[0, 0, 0, 0, 0, 0, 0, 0, 0],
        &[0, 1, 0, 0, 128, 0, 0, 0, 0],
        &[0, 1, 0, 0, 0, 0, 0, 7, 8],
    ] {
        assert!(Query::parse(bad).is_none());
    }
    let mut slot = BlockRead::EMPTY;
    for outcome in [None, Some(Outcome::Ack), Some(Outcome::NoAck)] {
        assert!(slot.admit(q, u32::MAX - 3));
        if let Some(outcome) = outcome {
            slot.sent(outcome, u32::MAX - 3);
        }
        slot.expire(WINDOW_MS - 5);
        assert!(slot.take_result(0).is_none());
        slot.expire(WINDOW_MS - 4);
        slot.sent(Outcome::Ack, 0);
        assert_eq!(
            &slot.take_result(0).unwrap().payload()[5..],
            &[0, 1, outcome.map_or(0xff, outcome_byte)]
        );
    }
    assert!(slot.admit(q, 0));
    slot.unavailable();
    assert_eq!(&slot.take_result(0).unwrap().payload()[5..], &[0, 2, 0xfe]);
    assert!(slot.admit(q, 0));
    slot.sent(Outcome::Late, 0);
    assert_eq!(&slot.take_result(0).unwrap().payload()[5..], &[0, 3, 5]);
    let mut max = [0xca; MAX_BYTES];
    max[..7].copy_from_slice(&[0x6e, 0x71, 0x20, 0x7f, 0xcd, 0xab, 0xff]);
    assert!(slot.admit(q, 0));
    slot.sent(Outcome::Ack, 0);
    slot.observe(4, &max, 9, |_| panic!());
    assert_eq!(&slot.take_result(4).unwrap().payload()[12..], &max);
}

#[test]
fn group_progress_deduplicates_logical_nodes_and_bounds_silent_peers() {
    let q = Query::parse(&[0x12, 1, 0x80, 0, 0x40, 0x38, 0, 7, 3]).unwrap();
    let mut slot = BlockRead::EMPTY;
    assert!(slot.admit(q, 0));
    assert_eq!(
        slot.frame(2),
        Request::new(
            2,
            &[
                0x6d, 0x70, 0, 11, 0x38, 0, 0x12, 1, 0x80, 0, 0x40, 3, 0, 0, 0
            ]
        )
    );
    slot.sent(Outcome::NoAck, WINDOW_MS - 1);
    let mut second = REPLY;
    second[2] = 0x24;
    second[3] = 5; // node 16
    let mut last = REPLY;
    last[2] = 0x27;
    last[3] = 0xc5; // node 31
    for (n, frame) in [REPLY, second].into_iter().enumerate() {
        let now = (2 + n as u32) * (WINDOW_MS - 1);
        slot.expire(now);
        assert!(slot.observe(2, &frame, now, |part| {
            assert_eq!(&part.payload()[5..8], &[2, 4, 1]);
            assert_eq!(&part.payload()[8..12], &now.to_le_bytes());
            assert_eq!(&part.payload()[12..], &frame);
            true
        }));
        let mut alias = frame;
        alias[2] ^= 8;
        slot.observe(2, &alias, now + 1, |_| panic!("duplicate node"));
        assert!(slot.take_result(2).is_none());
    }
    slot.expire(3 * (WINDOW_MS - 1) + WINDOW_MS);
    slot.observe(2, &last, 4 * WINDOW_MS, |_| panic!("after expiry"));
    assert_eq!(&slot.take_result(2).unwrap().payload()[5..], &[2, 1, 1]);

    assert!(slot.admit(q, 0));
    slot.sent(Outcome::Ack, 0);
    assert!(!slot.observe(2, &last, 10, |_| false));
    slot.observe(2, &second, 11, |_| panic!("after control overflow"));
    assert_eq!(&slot.take_result(2).unwrap().payload()[5..], &[2, 5, 0]);
    assert!(slot.take_result(2).is_none());
}

const EDIT: [u8; 14] = [0x12, 1, 3, 0, 0, 0, 7, 3, 1, 4, 9, 0, 4, 9];
const MODE: [u8; 10] = [0x6e, 0x71, 0x28, 0x46, 3, 0, 7, 1, 4, 9];

#[test]
fn guarded_edit_checks_the_entire_baseline_and_never_retries_a_write() {
    for (outcome, code) in [
        (Outcome::Ack, 0),
        (Outcome::NoAck, 1),
        (Outcome::ArbitrationLost, 2),
        (Outcome::EchoMismatch, 3),
        (Outcome::Late, 5),
    ] {
        let mut edit = BlockEdit::parse(&EDIT, 0).unwrap();
        edit.observe(&MODE);
        assert!(!edit.writing());
        edit.sent(Outcome::ArbitrationLost);
        assert_eq!(
            edit.frame(2),
            Request::new(2, &[0x6d, 0x70, 0, 11, 3, 0, 0x12, 1, 0, 0, 0, 0, 0, 0, 0])
        );
        edit.sent(Outcome::NoAck);
        edit.observe(&MODE);
        assert!(edit.writing());
        assert_eq!(
            edit.frame(2),
            Request::new(2, &[0x6d, 0xf0, 0, 7, 0x12, 1, 3, 0, 0, 4, 9])
        );
        edit.sent(outcome);
        assert!(edit.frame(2).is_none());
        assert_eq!(
            &edit.take_result(2).unwrap().payload()[5..],
            &[2, 0, 1, code]
        );
        assert!(edit.take_result(2).is_none());
    }
    for already_queued in [false, true] {
        let mut edit = BlockEdit::parse(&EDIT, 0).unwrap();
        edit.sent(Outcome::Ack);
        if already_queued {
            edit.observe(&MODE);
        }
        let mut changed = MODE;
        changed[9] ^= 1; // An untouched sibling changed: do not overwrite it.
        edit.observe(&changed);
        assert!(edit.frame(0).is_none());
        assert_eq!(
            &edit.take_result(0).unwrap().payload()[5..],
            &[0, 1, 0, 0xff]
        );
    }
}

#[test]
fn guarded_edit_never_updates_an_address_taken_or_vacated_after_admission() {
    let vacated = [0x7e, 0x79, 0x30, 0x82, 0x12, 0x21];
    let taken = [0x7e, 0x71, 0x28, 0x42, 0x43, 0x03];
    for moved in [vacated, taken] {
        for (guard_sent, update_queued) in [(false, false), (true, false), (true, true)] {
            let mut edit = BlockEdit::parse(&EDIT, 0).unwrap();
            if guard_sent {
                edit.sent(Outcome::Ack);
            }
            if update_queued {
                edit.observe(&MODE);
                assert!(edit.writing());
            }
            edit.observe(&moved);
            edit.observe(&MODE);
            assert!(edit.frame(2).is_none());
            assert_eq!(
                &edit.take_result(2).unwrap().payload()[5..],
                &[2, 5, if guard_sent { 0 } else { 0xff }, 0xff]
            );
        }
    }
    let mut edit = BlockEdit::parse(&EDIT, 0).unwrap();
    edit.sent(Outcome::NoAck);
    edit.observe(&MODE);
    edit.observe(&vacated[..5]);
    edit.observe(&[0x7e, 0x79, 0x30, 0x82, 0x43, 0x21]);
    assert!(edit.writing());
    edit.sent(Outcome::Ack);
    edit.observe(&taken);
    assert_eq!(&edit.take_result(2).unwrap().payload()[5..], &[2, 0, 1, 0]);
}

#[test]
fn malformed_unsupported_or_wrong_instance_edits_do_not_reach_the_bus() {
    for (offset, value) in [
        (1, 0),
        (1, 32),
        (2, 0),
        (4, 7),
        (6, 6),
        (6, 0x87),
        (7, 2),
        (7, 4),
        (11, 1),
    ] {
        let mut bad = EDIT;
        bad[offset] = value;
        assert!(BlockEdit::parse(&bad, 0).is_none(), "{offset}");
    }
    let hours = [0x12, 1, 0x44, 0, 0xff, 0x81, 7, 3, 0x81, 2, 1, 0x81, 0, 0];
    assert!(BlockEdit::parse(&hours, 0).is_some());
    let mut wrong = hours;
    wrong[11] = 1;
    assert!(BlockEdit::parse(&wrong, 0).is_none());
    let mut edit = BlockEdit::parse(&hours, 0).unwrap();
    edit.sent(Outcome::Ack);
    let mut frame = [0x6e, 0x71, 0x28, 0x46, 0x44, 0, 7, 1, 2, 1];
    edit.observe(&frame);
    assert!(!edit.writing());
    frame[7] = 0x81;
    edit.observe(&frame);
    assert!(edit.writing());
    edit.expire(WINDOW_MS);
    assert_eq!(
        &edit.take_result(2).unwrap().payload()[5..],
        &[2, 2, 0, 0xff]
    );
    assert_eq!(
        (0..=u16::MAX)
            .filter(|&b| edit::layout(b).is_some())
            .count(),
        55 // 54 explicit writers, and the documented zero-only tank runtime reset
    );
}
