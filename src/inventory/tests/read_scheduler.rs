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
