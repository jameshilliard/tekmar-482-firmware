use super::*;

fn at(ms: u32) -> Sample {
    Sample {
        seconds: ms / 1000,
        millis: (u32::MAX - 150).wrapping_add(ms),
    }
}

fn take(inv: &mut Inventory, p: usize, ms: u32) -> Option<[u8; 11]> {
    inv.next_probe(p, at(ms)).map(BlockProbe::body)
}

#[test]
fn water_cycle_does_not_hold_other_ports_or_host_transactions() {
    let mut inv = Inventory::new();
    observe(&mut inv, 4, 9, 0, 0x37f, &[0x81], 0);
    for p in [0, 1] {
        observe(&mut inv, p, 0, 1, 0x26f, &[0; 4], 0);
        assert_eq!(
            take(&mut inv, p, 500),
            Some([0x6d, 0x70, 0, 7, 13, 0, 0, 1, 0, 0, 0])
        );
    }
    // No reply yet to either line's probe; a host write can still complete.
    assert!(call(&mut inv, 0x127, 0, &[24, 0, 2], 0).is_none());
    expect_block_read(&mut inv, 3);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 7, 1, 4, 10], 0);
    assert_eq!(
        inv.command_frame(0),
        Request::new(0, &[0x6d, 0xf0, 0, 7, 0, 1, 3, 0, 2, 4, 10])
    );
    inv.command_sent(0, Outcome::Ack);
    expect_block_read(&mut inv, 3);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 7, 2, 4, 10], 0);
    assert_eq!(inv.command_reply(0).unwrap().payload()[7], 2);
    assert!(take(&mut inv, 1, 501).is_none());
    observe(&mut inv, 1, 0, 1, 0x2e7, &[13, 0, 1, 2], 0);
    assert_eq!(take(&mut inv, 1, 501).unwrap()[4], 0);
}

#[test]
fn reply_window_is_per_line_bounded_and_starts_at_physical_send_completion() {
    let mut inv = Inventory::new();
    for _ in 0..5 {
        observe(&mut inv, 2, 9, 24, 0x25f, &[0; 5], 0);
    }
    let probe = inv.next_probe(2, at(101)).unwrap();
    assert_eq!(probe.body(), [0x6d, 0x70, 0, 7, 0, 0, 9, 0, 0, 0x80, 0]);
    inv.probe_sent(probe, at(425).millis);
    // An unrelated block cannot release the wait for identity.
    observe(&mut inv, 2, 9, 24, 0x2e7, &[3, 0, 1, 2], 0);
    assert!(take(&mut inv, 2, 1_424).is_none());
    assert_eq!(take(&mut inv, 2, 1_425).unwrap()[4], 9);
    // Exhausted initial work stays exhausted even across many minutes.
    for ms in [2_500, 60_000, 120_000, 240_000] {
        assert!(take(&mut inv, 2, ms).is_none());
    }
}

#[test]
fn preempted_read_is_retained_but_cannot_follow_replacement_or_satisfied_field() {
    let mut inv = Inventory::new();
    for n in [1, 24] {
        observe(&mut inv, 2, 1, n, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
        observe(&mut inv, 2, 1, n, 0x26f, &[0; 4], 0);
    }
    let first = inv.next_probe(2, at(0)).unwrap();
    inv.defer_probe(first, 0); // host claimed the driver before SOF
    let next = inv.next_probe(2, at(1)).unwrap();
    assert_eq!(next.body(), [0x6d, 0x70, 0, 7, 13, 0, 1, 0, 0, 0x80, 0]); // fair to node 24
    observe(&mut inv, 2, 1, 24, 0x2e7, &[13, 0, 1, 2], 0);
    assert!(!inv.probe_current(next, 0));
    let retry = inv.next_probe(2, at(2)).unwrap();
    assert_eq!(retry.body(), first.body());
    for subnet in [9, 1] {
        observe(&mut inv, 2, subnet, 0, 0x3ff, &[], 0);
    }
    observe(&mut inv, 2, 1, 1, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    assert!(!inv.probe_current(retry, 0));
    inv.defer_probe(retry, 0);
    assert!(take(&mut inv, 2, 2).is_none());
}

#[test]
fn mode_hint_bypasses_cache_age_but_temperature_alone_does_not_query_mode() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    observe(&mut inv, 0, 0, 1, 0x36f, &[0, 1, 65, 0x20, 6, 0, 0, 0], 0);
    assert_eq!(take(&mut inv, 0, 1).unwrap()[4], 3);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, 1], 0);
    observe(&mut inv, 0, 0, 1, 0x36f, &[0, 1, 65, 0x21, 6, 0, 0, 0], 0);
    assert!(take(&mut inv, 0, 2).is_none());
    observe(&mut inv, 0, 0, 1, 0x36f, &[0, 3, 65, 0x21, 6, 0, 0, 0], 0);
    assert_eq!(
        take(&mut inv, 0, 3),
        Some([0x6d, 0x70, 0, 7, 3, 0, 0, 1, 0, 0, 0])
    );
}

#[test]
fn idle_enable_transitions_refresh_the_confirmed_mode_not_just_demand() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    observe(&mut inv, 0, 0, 1, 0x36f, &[0, 1, 65, 0x20, 6, 0, 0, 0], 0);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, 1], 0);
    // Off at an already satisfied temperature changes neither demand nor target.
    for (ms, flag, confirmed) in [(1, 0, 0), (2, 1, 1)] {
        for _ in 0..2 {
            observe(
                &mut inv,
                0,
                0,
                1,
                0x36f,
                &[0, flag, 65, 0x20, 6, 0, 0, 0],
                0,
            );
        }
        assert_eq!(take(&mut inv, 0, ms).unwrap()[4], 3);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, confirmed], 0);
        assert!(take(&mut inv, 0, ms).is_none());
    }
}

#[test]
fn changed_mode_overtakes_a_background_backlog_but_cannot_starve_it() {
    let mut inv = Inventory::new();
    for n in [1, 2, 3] {
        observe(&mut inv, 0, 0, n, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
        observe(&mut inv, 0, 0, n, 0x26f, &[0; 4], 0);
    }
    for n in [2, 3] {
        observe(&mut inv, 0, 0, n, 0x36f, &[0, 1, 65, 0x20, 6, 0, 0, 0], 0);
        observe(&mut inv, 0, 0, n, 0x2e7, &[3, 0, 1, 1], 0);
        observe(&mut inv, 0, 0, n, 0x36f, &[0, 0, 65, 0x20, 6, 0, 0, 0], 0);
    }
    for (ms, node, block) in [(0, 2, 3), (1, 1, 13), (2, 3, 3)] {
        let read = take(&mut inv, 0, ms).unwrap();
        assert_eq!((read[4], read[7]), (block, 1 << (node - 1)));
        assert!(take(&mut inv, 0, ms).is_none());
        observe(&mut inv, 0, 0, node, 0x2e7, &[block, 0, 1, 0], 0);
    }
}

#[test]
fn alternating_member_channels_preserve_the_other_channels_enable_bit() {
    let mut groups = air::AirGroups::EMPTY;
    let mut hint = 0;
    for (group, flag, expected) in [
        (1, 1, 0x21),
        (0x21, 1, 0x61),
        (1, 1, 0x61),
        (0x21, 0, 0x21),
        (2, 0, 2), // a different group must not inherit any channels
    ] {
        let status = groups
            .observe(0x36f, &[group, flag, 40, 0, 6, 0, 0, 0])
            .unwrap();
        hint = status.mode_hint(hint);
        assert_eq!(hint, expected);
    }
    let master = groups
        .observe(0x377, &[2, 0, 1, 40, 0, 6, 1, 80, 0, 6, 0, 0, 0])
        .unwrap();
    assert_eq!(master.mode_hint(hint), 0x62);
    let off = groups
        .observe(0x377, &[2, 0, 0, 40, 0, 6, 0, 80, 0, 6, 0, 0, 0])
        .unwrap();
    assert_eq!(off.mode_hint(0x62), 2);
}

#[test]
fn mode_reads_always_reply_but_only_changes_or_a_new_subscription_report() {
    let mut inv = Inventory::new();
    let report = |inv: &mut Inventory, mode| observe(inv, 0, 0, 1, 0x2e7, &[3, 0, 1, mode], 0);
    let push = |inv: &mut Inventory, mode| {
        assert_eq!(
            inv.next_report(0).unwrap().payload(),
            &[2, 0x27, 1, 0, 0, 24, 0, mode]
        )
    };
    report(&mut inv, 1);
    inv.set_reporting(true);
    report(&mut inv, 1);
    push(&mut inv, 1);
    for _ in 0..2 {
        assert!(call(&mut inv, 0x127, 1, &[24, 0], 0).is_none());
        expect_block_read(&mut inv, 3);
        report(&mut inv, 1);
        assert_eq!(
            inv.command_reply(0).unwrap().payload(),
            &[4, 0x27, 1, 0, 0, 24, 0, 1]
        );
        assert!(inv.next_report(0).is_none());
    }
    report(&mut inv, 0);
    push(&mut inv, 0);
    report(&mut inv, 1);
    report(&mut inv, 0); // coalesced before the pending report left the cache
    assert!(inv.next_report(0).is_none());
    inv.set_reporting(false);
    inv.set_reporting(true);
    report(&mut inv, 0);
    push(&mut inv, 0);
}

#[test]
fn an_expired_mode_request_keeps_its_failure_and_recovers_from_one_read_only_probe() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    inv.set_reporting(true);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, 1], 0);
    assert_eq!(inv.next_report(0).unwrap().payload()[7], 1);
    assert!(call(&mut inv, 0x127, 1, &[24, 0], 1).is_none());
    expect_block_read(&mut inv, 3);
    inv.command_expire(0, 11);
    assert_eq!(
        inv.command_reply(0).unwrap().payload(),
        &[4, 0x27, 1, 0, 0, 24, 0, 0xff]
    );
    assert!(inv.next_report(11).is_none());
    assert_eq!(
        take(&mut inv, 0, 11_000).unwrap(),
        [0x6d, 0x70, 0, 7, 3, 0, 0, 1, 0, 0, 0]
    );
    assert!(take(&mut inv, 0, 12_000).is_none()); // no timer-driven retry loop
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, 1], 12);
    assert_eq!(
        inv.next_report(12).unwrap().payload(),
        &[2, 0x27, 1, 0, 0, 24, 0, 1]
    );
    assert!(call(&mut inv, 0x127, 0, &[24, 0, 2], 13).is_none());
    inv.command_expire(0, 23);
    assert_eq!(inv.command_reply(0).unwrap().payload()[0], 3);
    assert!(take(&mut inv, 0, 24_000).is_none()); // never replay a failed write
}

#[test]
fn identifying_a_setpoint_controller_retires_unsent_thermostat_reads() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], 0);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, 1], 0);
    let heat = inv.next_probe(0, at(0)).unwrap();
    assert_eq!(heat.body()[4], 5);
    observe(
        &mut inv,
        0,
        0,
        1,
        0x22f,
        &[0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        0,
    );
    assert!(!inv.probe_current(heat, 0));
    inv.defer_probe(heat, 0);
    assert_eq!(take(&mut inv, 0, 1).unwrap()[4], 0x26);
    assert!(take(&mut inv, 0, 1_001).is_none());
}

#[test]
fn repeated_unanswered_heating_read_cannot_starve_cooling_on_the_same_node() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0, 0, 63, 1, 2, 3, 4, 5, 6], 0);
    observe(&mut inv, 0, 0, 1, 0x377, &[0; 13], 0);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, 2], 0);
    assert_eq!(take(&mut inv, 0, 0).unwrap()[4], 5);
    observe(&mut inv, 0, 0, 1, 0x377, &[0; 13], 1);
    assert_eq!(take(&mut inv, 0, 1_000).unwrap()[4], 6);
}
