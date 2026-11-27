use super::*;

fn request(inv: &mut Inventory, address: u16, now: u32) -> Option<Reply> {
    call(inv, 0x127, 1, &address.to_le_bytes(), now)
}

fn answer(inv: &mut Inventory, address: u16, mode: u8) {
    let [a, b] = address.to_le_bytes();
    assert_eq!(
        inv.next_mode_reply().unwrap().payload(),
        &[4, 0x27, 1, 0, 0, a, b, mode]
    );
}

fn mode(inv: &mut Inventory, port: usize, node: u8, value: u8, now: u32) {
    observe(inv, port, 0, node, 0x2e7, &[3, 0, 7, value, 4, 1], now);
}

#[test]
fn independent_reads_share_peer_evidence_not_a_port_or_an_unsolicited_report() {
    let mut inv = Inventory::new(); // Reporting disabled throughout.
    for (p, n) in [(0, 1), (0, 2), (1, 1)] {
        observe(&mut inv, p, 0, n, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    }
    // A silent node must not hold replies for peers on either port.
    for address in [24, 23, 23, 1024] {
        assert!(request(&mut inv, address, 0).is_none());
    }
    assert!(!inv.has_command(0));
    mode(&mut inv, 1, 1, 0, 1);
    answer(&mut inv, 1024, 0);
    mode(&mut inv, 0, 2, 2, 1);
    // A new, already-confirmed read cannot overtake the undelivered responses.
    assert!(request(&mut inv, 23, 1).is_none());
    for _ in 0..3 {
        answer(&mut inv, 23, 2);
    }
    assert!(inv.next_mode_reply().is_none());
    assert!(inv.next_report(1).is_none());
    assert!(call(&mut inv, 0x13f, 1, &[24, 0, 0], 1).is_none()); // unrelated foreground job
    assert_eq!(request(&mut inv, 23, 1).unwrap().payload()[7], 2);
    assert!(request(&mut inv, 24, 9).is_none());
    inv.tick(10);
    mode(&mut inv, 0, 1, 1, 10); // Too late for either waiter.
    assert!(request(&mut inv, 24, 10).is_none());
    answer(&mut inv, 24, 255);
    answer(&mut inv, 24, 255);
    answer(&mut inv, 24, 1);
    assert!(!inv.has_mode_requests());
}

#[test]
fn a_mode_write_baseline_and_link_ack_cannot_resolve_later_reads() {
    let mut inv = Inventory::new();
    mode(&mut inv, 0, 1, 0, 0);
    inv.set_reporting(true, 0);
    assert!(call(&mut inv, 0x127, 0, &[24, 0, 2], 0).is_none());
    assert!(!inv.begin_native_mode_edit(0, 0, 1, 0));
    assert!(request(&mut inv, 24, 0).is_none());
    expect_block_read(&mut inv, 3);
    mode(&mut inv, 0, 1, 0, 1);
    assert!(inv.next_mode_reply().is_none());
    assert!(inv.next_report(1).is_none());
    assert_eq!(
        inv.command_frame(0),
        Request::new(0, &[0x6d, 0xf0, 0, 7, 0, 1, 3, 0, 2, 4, 1])
    );
    inv.command_sent(0, Outcome::Ack);
    mode(&mut inv, 0, 1, 2, 2); // No verification request has gone out.
    assert!(inv.next_mode_reply().is_none());
    expect_block_read(&mut inv, 3);
    mode(&mut inv, 0, 1, 0, 3); // Device retained off.
    assert_eq!(inv.command_reply(0).unwrap().payload()[7], 0);
    answer(&mut inv, 24, 0);
    assert_eq!(inv.next_report(3).unwrap().payload()[7], 0);
}

#[test]
fn replies_precede_a_newer_push_for_that_address_without_holding_another_thermostat() {
    let mut inv = Inventory::new();
    inv.set_reporting(true, 0);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    for _ in 0..2 {
        assert!(request(&mut inv, 24, 0).is_none());
    }
    inv.tick(10);
    mode(&mut inv, 0, 1, 1, 10);
    mode(&mut inv, 0, 2, 0, 10);
    assert_eq!(
        inv.next_report(10).unwrap().payload(),
        &[2, 0x27, 1, 0, 0, 23, 0, 0]
    );
    answer(&mut inv, 24, 255);
    assert!(inv.next_report(10).is_none());
    answer(&mut inv, 24, 255);
    assert_eq!(
        inv.next_report(10).unwrap().payload(),
        &[2, 0x27, 1, 0, 0, 24, 0, 1]
    );
}

#[test]
fn native_edits_and_external_updates_invalidate_only_the_addressed_thermostat() {
    let mut inv = Inventory::new();
    for n in [1, 2] {
        mode(&mut inv, 0, n, 2, 0);
    }
    assert!(inv.begin_native_mode_edit(0, 0, 1, 0));
    assert_eq!(
        call(&mut inv, 0x127, 0, &[24, 0, 1], 0).unwrap().payload()[9],
        BridgeError::Busy as u8
    );
    assert!(request(&mut inv, 24, 0).is_none());
    mode(&mut inv, 0, 1, 2, 1); // Guarded edit baseline, not peer readback.
    assert!(inv.next_mode_reply().is_none());
    inv.end_native_edit(0, 1);
    mode(&mut inv, 0, 1, 1, 2);
    answer(&mut inv, 24, 1);
    for payload in [
        &[8, 2, 3, 0, 0][..],
        &[0, 2, 3, 1, 0],
        &[0, 2, 3, 0],
        &[0, 34, 3, 0, 0],
    ] {
        observe(&mut inv, 0, 0, 1, 0x2df, payload, 3);
        assert_eq!(request(&mut inv, 23, 3).unwrap().payload()[7], 2);
    }
    observe(&mut inv, 0, 0, 1, 0x2df, &[0, 2, 3, 0, 0, 4, 1], 3);
    assert!(request(&mut inv, 23, 3).is_none());
    assert_eq!(request(&mut inv, 24, 3).unwrap().payload()[7], 1);
    mode(&mut inv, 0, 2, 2, 4); // The written value was not accepted.
    answer(&mut inv, 23, 2);
}

#[test]
fn age_change_hints_and_address_reuse_never_extend_a_cached_confirmation() {
    let mut inv = Inventory::new();
    let start = u32::MAX - 120;
    let at = |s: u32| start.wrapping_add(s);
    let air = |inv: &mut Inventory, enable, temp, now| {
        observe(
            inv,
            0,
            0,
            1,
            0x36f,
            &[0, enable, 65, temp, 6, 0, 0, 0],
            at(now),
        )
    };
    air(&mut inv, 1, 0, 0);
    mode(&mut inv, 0, 1, 1, at(0));
    for s in [100, 200, 299] {
        air(&mut inv, 1, s as u8, s);
        assert_eq!(request(&mut inv, 24, at(s)).unwrap().payload()[7], 1);
    }
    air(&mut inv, 1, 42, 300);
    assert!(request(&mut inv, 24, at(300)).is_none());
    mode(&mut inv, 0, 1, 2, at(301));
    answer(&mut inv, 24, 2);
    air(&mut inv, 0, 42, 301);
    assert!(request(&mut inv, 24, at(301)).is_none());
    observe(&mut inv, 0, 0, 2, 0x3e7, &[0, 1], at(302));
    assert!(request(&mut inv, 23, at(302)).is_none()); // moved row needs fresh evidence
    mode(&mut inv, 0, 1, 0, at(302)); // new occupant of old address
    mode(&mut inv, 0, 2, 1, at(302));
    answer(&mut inv, 24, 255);
    answer(&mut inv, 23, 1);
}

#[test]
fn reporting_enable_seeds_only_present_current_modes_without_reading_the_bus() {
    let mut inv = Inventory::new();
    for n in [1, 2, 3] {
        mode(&mut inv, 0, n, 1, 0);
    }
    for s in [120, 240, 300] {
        // Non-mode traffic keeps the third device present but does not renew its mode.
        observe(&mut inv, 0, 0, 3, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], s);
    }
    mode(&mut inv, 0, 1, 2, 300);
    inv.set_reporting(true, 300);
    assert_eq!(
        inv.next_report(300).unwrap().payload(),
        &[2, 0x27, 1, 0, 0, 24, 0, 2]
    );
    inv.set_reporting(true, 300);
    assert!(inv.next_report(300).is_none());
    assert!(request(&mut inv, 22, 300).is_none());
    assert_eq!(
        inv.next_probe(
            0,
            Sample {
                seconds: 300,
                millis: 300_000
            }
        )
        .unwrap()
        .body(),
        [0x6d, 0x70, 0, 7, 3, 0, 0, 4, 0, 0, 0]
    );
}

#[test]
fn capacity_rejects_excess_reads_without_losing_replies_or_issuing_extra_probes() {
    let mut inv = Inventory::new();
    for n in 1..=24 {
        observe(&mut inv, 0, 0, n, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
        assert!(request(&mut inv, 25 - u16::from(n), 0).is_none());
    }
    observe(&mut inv, 1, 0, 1, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    assert_eq!(
        request(&mut inv, 1024, 0).unwrap().payload()[9],
        BridgeError::Busy as u8
    );
    for _ in 1..255 {
        assert!(request(&mut inv, 24, 0).is_none());
    }
    assert_eq!(
        request(&mut inv, 24, 0).unwrap().payload()[9],
        BridgeError::Busy as u8
    );
    assert_eq!(
        inv.next_probe(
            0,
            Sample {
                seconds: 0,
                millis: 0
            }
        )
        .unwrap()
        .body(),
        [0x6d, 0x70, 0, 7, 3, 0, 0, 1, 0, 0, 0]
    );
    mode(&mut inv, 0, 1, 1, 0);
    for _ in 0..255 {
        answer(&mut inv, 24, 1);
    }
    assert!(inv.next_mode_reply().is_none());
    assert!(request(&mut inv, 1024, 0).is_none()); // Capacity was released.
    call(&mut inv, INVENTORY, 0, &[0, 0], 0);
    for address in (1..24).rev() {
        answer(&mut inv, address, 255);
    }
    answer(&mut inv, 1024, 255);
    assert!(!inv.has_mode_requests());
}
