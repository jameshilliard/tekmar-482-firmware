use super::*;
const KEY: [u8; 6] = [0x12, 1, 0x38, 0, 7, 3];
const REPLY: [u8; 9] = [0x6e, 0x71, 0x28, 0x45, 0x38, 0, 3, 0xf3, 0xca];

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
    slot.sent(Outcome::ArbitrationLost);
    slot.observe(&REPLY, 51);
    assert!(slot.take_result(2).is_none() && slot.queued());
    slot.sent(Outcome::NoAck);
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
        slot.observe(&other, 54);
        assert!(slot.take_result(2).is_none());
    }
    assert!(!q.matches(&[0x6e, 0x71, 0x28, 0x46, 0x38, 0, 0x81, 3, 4, 5]));
    slot.observe(&REPLY, 56);
    assert!(!slot.admit(q, 57));
    let result = slot.take_result(2).unwrap();
    assert_eq!(
        &result.payload()[..11],
        &[2, 15, 0xf0, 0x4b, 0x54, 2, 0, 56, 0, 0, 0]
    );
    assert_eq!(&result.payload()[11..], &REPLY);
    assert!(!slot.active() && slot.take_result(2).is_none());
}

#[test]
fn expiry_failures_and_maximum_extension_never_produce_cached_data() {
    let q = Query::parse(&[0x12, 1, 0xcd, 0xab, 0, 0]).unwrap();
    for bad in [
        &KEY[..5],
        &[0, 0, 0, 0, 0, 0],
        &[0, 32, 0, 0, 0, 0],
        &[0, 1, 0, 0, 7, 8],
    ] {
        assert!(Query::parse(bad).is_none());
    }
    let mut slot = BlockRead::EMPTY;
    for outcome in [None, Some(Outcome::Ack), Some(Outcome::NoAck)] {
        assert!(slot.admit(q, u32::MAX - 3));
        if let Some(outcome) = outcome {
            slot.sent(outcome);
        }
        slot.expire(WINDOW_MS - 5);
        assert!(slot.take_result(0).is_none());
        slot.expire(WINDOW_MS - 4);
        slot.sent(Outcome::Ack);
        assert_eq!(&slot.take_result(0).unwrap().payload()[5..], &[0, 1]);
    }
    assert!(slot.admit(q, 0));
    slot.unavailable();
    assert_eq!(&slot.take_result(0).unwrap().payload()[5..], &[0, 2]);
    assert!(slot.admit(q, 0));
    slot.sent(Outcome::Late);
    assert_eq!(&slot.take_result(0).unwrap().payload()[5..], &[0, 3]);
    let mut max = [0xca; MAX_BYTES];
    max[..7].copy_from_slice(&[0x6e, 0x71, 0x20, 0x7f, 0xcd, 0xab, 0xff]);
    assert!(slot.admit(q, 0));
    slot.sent(Outcome::Ack);
    slot.observe(&max, 9);
    assert_eq!(&slot.take_result(4).unwrap().payload()[11..], &max);
}
