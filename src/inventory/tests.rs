use super::*;

fn observe(
    cache: &mut Inventory,
    port: usize,
    subnet: u8,
    node: u8,
    method: u32,
    body: &[u8],
    now: u32,
) -> Option<[u8; 11]> {
    let mut frame = [0; 67];
    let h = method << 20 | u32::from(subnet) << 12 | u32::from(node) << 6 | body.len() as u32;
    frame[..4].copy_from_slice(&h.to_be_bytes());
    frame[4..4 + body.len()].copy_from_slice(body);
    match cache.observe(port, &frame[..4 + body.len()], now, true)? {
        Observation::Probe(body) => Some(body),
        Observation::Moved(_) => None,
    }
}

fn call(cache: &mut Inventory, method: u32, service: u8, body: &[u8], now: u32) -> Option<Reply> {
    match cache.handle(
        &Call {
            method,
            service,
            body,
        },
        now,
    ) {
        Answer::Reply(r) => Some(r),
        Answer::Silent => None,
        _ => panic!(),
    }
}

fn address(reply: Reply) -> u16 {
    u16::from_le_bytes(reply.payload()[5..].try_into().unwrap())
}

#[test]
fn taking_address_is_report_only_and_follows_the_known_device() {
    let mut inv = Inventory::new(); // Reporting disabled.
    let move_to = |inv: &mut Inventory, subnet: u8, old: [u8; 2], now| {
        let header = 0x3e7_00000 | u32::from(subnet) << 12 | 23 << 6 | 2;
        let [a, b, c, d] = header.to_be_bytes();
        match inv.observe(0, &[a, b, c, d, old[0], old[1]], now, false)? {
            Observation::Moved(moved) => Some(moved.report()),
            _ => panic!("an address change must not trigger a probe"),
        }
    };
    observe(&mut inv, 0, 0, 24, 0x2e7, &[0, 0, 7, 7, 8, 9], 0);
    observe(&mut inv, 0, 0, 23, 0x2e7, &[0, 0, 7, 1, 2, 3], 0);
    assert!(move_to(&mut inv, 0, [8, 24], 1).is_none()); // Same display subnet, different wire subnet.
    assert_eq!(
        move_to(&mut inv, 0, [0, 56], 1).unwrap().payload(),
        &[2, 0x5f, 1, 0, 0, 1, 0, 2, 0]
    );
    assert!(move_to(&mut inv, 0, [0, 24], 1).is_none()); // Duplicate.
    assert_eq!(
        move_to(&mut inv, 9, [0, 23], 2).unwrap().payload()[5..],
        [2, 0, 102, 0]
    );
    assert_eq!(
        address(call(&mut inv, INVENTORY, 1, &[2, 0], 2).unwrap()),
        u16::MAX
    );
    assert_eq!(
        call(&mut inv, PRODUCT, 1, &[102, 0], 2).unwrap().payload()[7..],
        [7, 8, 9, 0]
    );
    call(&mut inv, 0x127, 1, &[102, 0], 2);
    assert_eq!(
        inv.command_frame(0),
        Request::new(0, &[0x6d, 0x70, 0, 7, 3, 0, 9, 0, 0, 64, 0])
    );
    assert!(move_to(&mut inv, 0, [9, 23], 153).is_none()); // Expired.
    for service in [0, 1] {
        assert!(call(&mut inv, TAKING_ADDRESS, service, &[1, 0, 2, 0], 0).is_none());
    }
}

#[test]
fn group_read_is_global_and_an_expired_enable_is_not_a_stop_report() {
    let mut inv = Inventory::new();
    let initial = u32::MAX - 20;
    let read =
        |inv: &mut Inventory, id, now| call(inv, 0x13d, 1, &[id, 0], now).unwrap().payload()[6];
    assert_eq!(read(&mut inv, 1, initial), 0);
    assert_eq!(read(&mut inv, 13, initial), 255);
    observe(&mut inv, 4, 79, 0, 0x20f, &[1, 16], initial);
    assert!(!inv.has_reports());
    assert_eq!(read(&mut inv, 1, 38), 1); // 59 seconds across wrap.
    inv.set_reporting(true);
    observe(&mut inv, 2, 14, 0, 0x20f, &[1, 1], 38);
    assert_eq!(
        inv.next_report(38).unwrap().payload(),
        &[2, 0x3d, 1, 0, 0, 1, 0]
    );
    observe(&mut inv, 2, 14, 0, 0x20f, &[1, 16], 40);
    assert!(inv.next_report(100).is_none());
    assert_eq!(read(&mut inv, 1, 100), 0);
    assert_eq!(
        address(call(&mut inv, INVENTORY, 1, &[0, 0], 100).unwrap()),
        0
    );
    assert_eq!(
        call(&mut inv, 0x13d, 0, &[0, 1], 100).unwrap().payload()[6],
        255
    );
    assert_eq!(
        call(&mut inv, 0x13d, 0, &[12, 255], 100).unwrap().payload()[6],
        255
    );
    assert!((0..5).all(|p| !inv.has_group_send(p)));
    observe(&mut inv, 0, 0, 0, 0x20f, &[12, 16], 100);
    inv.set_reporting(false);
    assert!(!inv.has_reports());
    assert_eq!(read(&mut inv, 12, 100), 1);
    inv.reset();
    assert_eq!(read(&mut inv, 12, 100), 0);
}

#[test]
fn broadcasts_send_once_per_line_replace_unsent_group_and_expire() {
    let mut inv = Inventory::new();
    for id in 1..=12 {
        assert!(call(&mut inv, 0x13d, 0, &[id, 1], 10).is_none());
    }
    let first = inv.next_group_send(0, 11).unwrap();
    assert_eq!(first.frame(0), Request::new(0, &[0x61, 0xf0, 0, 2, 1, 1]));
    assert!(!inv.stale_group_send(0, first, 11));
    call(&mut inv, 0x13d, 0, &[1, 0], 12);
    assert!(inv.stale_group_send(0, first, 12));
    for p in 0..5 {
        for id in (2..=12).chain([1]) {
            let frame = inv.next_group_send(p, 13).unwrap().frame(p as u8);
            assert_eq!(
                frame,
                Request::new(p as u8, &[0x61, 0xf0, 0, 2, id, u8::from(id != 1)])
            );
        }
        assert!(!inv.has_group_send(p));
    }
    call(&mut inv, 0x13d, 0, &[1, 1], u32::MAX - 3);
    let tx = inv.next_group_send(2, 1).unwrap();
    assert!(!inv.stale_group_send(2, tx, 5));
    assert!(inv.stale_group_send(2, tx, 6));
    assert!(inv.next_group_send(0, 6).is_none());
    assert!((0..5).all(|p| !inv.has_group_send(p)));
}

#[test]
fn schedule_events_only_describe_an_observed_schedule_master() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 24, 0x2f7, &[0x84, 1, 60], 0);
    assert!(call(&mut inv, 0x17f, 1, &[1, 0, 0], 0).is_none());
    observe(&mut inv, 0, 0, 24, 0x36f, &[0; 8], 0);
    inv.set_reporting(true);
    for (flags, code) in [(4, 0), (0x44, 1), (0x84, 2), (0xc4, 255), (0x80, 255)] {
        observe(&mut inv, 0, 0, 24, 0x2f7, &[flags, 1, 60], 1);
        assert_eq!(
            inv.next_report(1).unwrap().payload(),
            &[2, 0x7f, 1, 0, 0, 1, 0, code]
        );
        assert_eq!(
            call(&mut inv, 0x17f, 0, &[1, 0, 77], 1).unwrap().payload(),
            &[3, 0x7f, 1, 0, 0, 1, 0, code]
        );
        assert!(!inv.has_command(0));
    }
}

fn expect_block_read(inv: &mut Inventory, block: u8) {
    assert_eq!(
        inv.command_frame(0),
        Request::new(0, &[0x6d, 0x70, 0, 7, block, 0, 0, 1, 0, 0, 0])
    );
    inv.command_sent(0, true);
}

#[test]
fn slab_requires_enabled_floor_sensor_and_preserves_maximum_in_all_slot_updates() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], 0);
    for (setup, allowed) in [
        (&[13, 0, 1, 1][..], false),
        (&[13, 0, 3, 1, 2], true),
        (&[13, 0, 1, 2], true),
    ] {
        assert!(call(&mut inv, 0x14f, 0, &[24, 0, 7, 42], 1).is_none());
        expect_block_read(&mut inv, 13);
        observe(&mut inv, 0, 0, 1, 0x2e7, setup, 1);
        if !allowed {
            assert!(inv.command_frame(0).is_none());
            assert_eq!(inv.command_reply(0).unwrap().payload()[8], 255);
            continue;
        }
        expect_block_read(&mut inv, 7);
        observe(
            &mut inv,
            0,
            0,
            1,
            0x2e7,
            &[7, 0, 63, 10, 20, 30, 40, 50, 70],
            1,
        );
        assert_eq!(
            inv.command_frame(0),
            Request::new(0, &[0x6d, 0xf0, 0, 10, 0, 1, 7, 0, 42, 42, 42, 42, 42, 70])
        );
        inv.command_sent(0, true);
        expect_block_read(&mut inv, 7);
        observe(
            &mut inv,
            0,
            0,
            1,
            0x2e7,
            &[7, 0, 63, 41, 41, 41, 41, 41, 70],
            1,
        );
        assert_eq!(
            inv.command_reply(0).unwrap().payload(),
            &[3, 0x4f, 1, 0, 0, 24, 0, 0, 41]
        );
    }
    inv.set_reporting(true);
    observe(&mut inv, 0, 0, 1, 0x26f, &[1, 44, 10, 0], 1);
    assert!(
        core::iter::from_fn(|| inv.next_report(1))
            .any(|r| r.payload() == [2, 0x4f, 1, 0, 0, 24, 0, 0, 44])
    );
    observe(&mut inv, 0, 0, 1, 0x26f, &[2, 70, 10, 0], 1);
    assert!(!core::iter::from_fn(|| inv.next_report(1)).any(|r| r.payload()[1] == 0x4f));
}

#[test]
fn fan_uses_ventilation_block_not_duct_status_or_an_unoccupied_away_alias() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], 0);
    let q = observe(&mut inv, 0, 0, 1, 0x25f, &[0x33, 0, 0, 0, 0], 0).unwrap();
    assert_eq!(q[4], 9);
    assert!(observe(&mut inv, 0, 0, 1, 0x25f, &[0; 5], 1).is_none());
    call(&mut inv, 0x16f, 0, &[1], 1);
    assert!(call(&mut inv, 0x157, 0, &[24, 0, 6, 60], 1).is_none());
    expect_block_read(&mut inv, 9);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[9, 0, 31, 10, 20, 30, 40, 50], 1);
    assert_eq!(
        inv.command_frame(0),
        Request::new(0, &[0x6d, 0xf0, 0, 9, 0, 1, 9, 0, 10, 20, 30, 40, 60])
    );
    inv.command_sent(0, true);
    expect_block_read(&mut inv, 9);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[9, 0, 31, 10, 20, 30, 40, 55], 1);
    assert_eq!(
        &inv.command_reply(0).unwrap().payload()[5..],
        &[24, 0, 6, 55]
    );
    let [p, q, r, _] = 99203u32.to_le_bytes();
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0, 0, 7, p, q, r], 1);
    assert_eq!(
        call(&mut inv, 0x157, 0, &[24, 0, 6, 11], 1)
            .unwrap()
            .payload()[8],
        255
    );
    assert!(!inv.has_command(0));
}

#[test]
fn humidity_limits_require_rh_report_and_the_corresponding_setup_flag() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], 0);
    assert_eq!(
        call(&mut inv, 0x151, 0, &[24, 0, 40], 1).unwrap().payload()[7],
        255
    );
    observe(&mut inv, 0, 0, 1, 0x323, &[0, 0, 41, 0, 0, 0], 1);
    for (method, block, mask, enabled) in [
        (0x151, 0xa5, 1, false),
        (0x152, 0xa4, 2, false),
        (0x151, 0xa5, 2, true),
        (0x152, 0xa4, 1, true),
    ] {
        assert!(call(&mut inv, method, 0, &[24, 0, 40], 1).is_none());
        expect_block_read(&mut inv, 0xa3);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[0xa3, 0, mask, 1], 1);
        if !enabled {
            assert!(inv.command_frame(0).is_none());
            assert_eq!(inv.command_reply(0).unwrap().payload()[7], 255);
            continue;
        }
        expect_block_read(&mut inv, block);
        observe(
            &mut inv,
            0,
            0,
            1,
            0x2e7,
            &[block, 0, 31, 32, 33, 34, 35, 36],
            1,
        );
        assert_eq!(
            inv.command_frame(0),
            Request::new(0, &[0x6d, 0xf0, 0, 9, 0, 1, block, 0, 40, 33, 34, 35, 36])
        );
        inv.command_sent(0, true);
        expect_block_read(&mut inv, block);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[block, 0, 1, 38], 1);
        assert_eq!(&inv.command_reply(0).unwrap().payload()[5..], &[24, 0, 38]);
    }
}

#[test]
fn mode_write_preserves_siblings_and_confirms_only_from_postwrite_read() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 8, 24, 0x36f, &[0; 8], 0);
    assert!(call(&mut inv, 0x127, 0, &[1, 0, 2, 0], 0).is_none());
    let read = Request::new(0, &[0x6d, 0x70, 0, 7, 3, 0, 8, 0, 0, 128, 0]);
    assert_eq!(inv.command_frame(0), read);
    let sample = |inv: &mut Inventory, mode| {
        observe(inv, 0, 8, 24, 0x2e7, &[3, 0, 7, mode, 3, 9], 1);
    };
    sample(&mut inv, 1); // Before our request has left the line.
    inv.command_sent(0, true);
    observe(&mut inv, 1, 8, 24, 0x2e7, &[3, 0, 7, 1, 3, 9], 1);
    observe(&mut inv, 0, 0, 24, 0x2e7, &[3, 0, 7, 1, 3, 9], 1);
    observe(&mut inv, 0, 8, 23, 0x2e7, &[3, 0, 7, 1, 3, 9], 1);
    observe(&mut inv, 0, 8, 24, 0x2e7, &[4, 0, 7, 1, 3, 9], 1);
    assert!(inv.command_frame(0).is_none());
    sample(&mut inv, 1);
    assert_eq!(
        inv.command_frame(0),
        Request::new(0, &[0x6d, 0xf0, 0, 7, 8, 24, 3, 0, 2, 3, 9])
    );
    sample(&mut inv, 2);
    inv.command_sent(0, true);
    assert_eq!(inv.command_frame(0), read);
    sample(&mut inv, 2);
    inv.command_sent(0, true);
    assert!(inv.command_reply(0).is_none());
    sample(&mut inv, 1); // Device kept its previous mode.
    assert_eq!(
        inv.command_reply(0).unwrap().payload(),
        &[3, 0x27, 1, 0, 0, 1, 0, 1]
    );
    assert!(!inv.has_command(0));
}

#[test]
fn setpoints_resolve_current_preserve_slots_and_report_observed_values() {
    let mut inv = Inventory::new();
    for (enable, selector, slot, want) in [
        (1, 7, 3, [10, 99, 12, 99, 14]),
        (1, 4, 4, [99, 11, 99, 13, 14]),
        (1, 6, 6, [10, 11, 12, 13, 99]),
        (0, 2, 2, [99; 5]),
    ] {
        call(&mut inv, 0x16f, 0, &[enable], 0);
        observe(&mut inv, 0, 0, 1, 0x36f, &[0, 12, 0, 0, 0, 0, 0, 0], 0);
        assert!(call(&mut inv, 0x13f, 0, &[24, 0, selector, 99], 0).is_none());
        assert!(inv.command_frame(0).is_some());
        inv.command_sent(0, true);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[5, 0, 31, 10, 11, 12, 13, 14], 0);
        let mut bytes = [0x6d, 0xf0, 0, 9, 0, 1, 5, 0, 0, 0, 0, 0, 0];
        bytes[8..].copy_from_slice(&want);
        assert_eq!(inv.command_frame(0), Request::new(0, &bytes));
        inv.command_sent(0, true);
        assert!(inv.command_frame(0).is_some());
        inv.command_sent(0, true);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[5, 0, 31, 88, 88, 88, 88, 88], 0);
        assert_eq!(
            &inv.command_reply(0).unwrap().payload()[5..],
            &[24, 0, slot, 88]
        );
    }
    inv.set_reporting(true);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[6, 0, 31, 10, 11, 12, 13, 14], 0);
    assert_eq!(
        inv.next_report(0).unwrap().payload(),
        &[2, 0x47, 1, 0, 0, 24, 0, 3, 11]
    );
}

#[test]
fn unsupported_ambiguous_and_abandoned_operations_never_generate_an_update() {
    let mut inv = Inventory::new();
    let start = u32::MAX - 2;
    observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], start);
    let begin = |inv: &mut Inventory| {
        assert!(call(inv, 0x127, 0, &[24, 0, 1], start).is_none());
        assert!(inv.command_frame(0).is_some());
        inv.command_sent(0, true);
    };
    for (block, out) in [(&[3, 0, 1, 2][..], 255), (&[3, 0, 7, 1, 3, 9], 1)] {
        begin(&mut inv);
        observe(&mut inv, 0, 0, 1, 0x2e7, block, start);
        assert!(inv.command_frame(0).is_none());
        assert_eq!(inv.command_reply(0).unwrap().payload()[7], out);
    }
    begin(&mut inv);
    assert!(call(&mut inv, 0x127, 1, &[24, 0], start).is_some()); // Same port busy.
    inv.command_expire(0, 7);
    assert_eq!(inv.command_reply(0).unwrap().payload()[7], 255);
    begin(&mut inv);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 7, 2, 3, 9], start);
    observe(&mut inv, 0, 0, 2, 0x3e7, &[0, 1], start);
    assert!(inv.command_frame(0).is_none());
    assert_eq!(inv.command_reply(0).unwrap().payload()[7], 255);
    observe(
        &mut inv,
        0,
        0,
        2,
        0x22f,
        &[0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        start,
    );
    assert_eq!(
        call(&mut inv, 0x127, 0, &[23, 0, 1], start)
            .unwrap()
            .payload()[7],
        255
    );
    assert!(!inv.has_command(0));
}

#[test]
fn full_walk_is_a_snapshot_with_one_terminator() {
    let mut inv = Inventory::default();
    for p in 0..PORTS {
        for n in 1..=24 {
            observe(&mut inv, p, 0, n, 0x36f, &[0; 8], 9);
        }
    }
    assert_eq!(
        address(call(&mut inv, INVENTORY, 1, &[0, 0], 9).unwrap()),
        24
    );
    observe(&mut inv, 4, 9, 31, 0x3ff, &[0], 10); // later port change cannot truncate walk
    for p in 0..PORTS {
        for n in (1..=24).rev() {
            if (p, n) != (0, 24) {
                assert_eq!(address(inv.walk_next().unwrap()), p as u16 * 1000 + n);
            }
        }
    }
    assert_eq!(address(inv.walk_next().unwrap()), 0);
    assert!(inv.walk_next().is_none());
    assert_eq!(
        address(call(&mut inv, INVENTORY, 1, &4001u16.to_le_bytes(), 10).unwrap()),
        u16::MAX
    );
}

#[test]
fn identity_handles_partial_masks_clock_wrap_and_reuse() {
    let mut inv = Inventory::default();
    let start = u32::MAX - 10;
    let q = observe(&mut inv, 2, 0x11, 33, 0x36f, &[0; 8], start).unwrap();
    assert_eq!(q, [0x6d, 0x70, 0, 7, 0, 0, 0x11, 1, 0, 0, 0]);
    let key = 2124u16.to_le_bytes();
    let val = |inv: &mut Inventory, m, now| {
        u32::from_le_bytes(
            call(inv, m, 1, &key, now).unwrap().payload()[7..]
                .try_into()
                .unwrap(),
        )
    };
    assert_eq!(val(&mut inv, PRODUCT, start), UNKNOWN);
    assert!(observe(&mut inv, 2, 0x11, 1, 0x2e7, &[0, 0, 7, 1, 2, 3], 4).is_none());
    assert_eq!(val(&mut inv, PRODUCT, 4), 0x030201);
    observe(
        &mut inv,
        2,
        0x11,
        1,
        0x2e7,
        &[0, 0, 0x38, 0xff, 0xff, 0xff, 0x23, 1, 4],
        5,
    );
    assert_eq!(val(&mut inv, VERSION, 5), 29104);
    assert!(observe(&mut inv, 2, 0x11, 1, 0x36f, &[0; 8], 100).is_none());
    assert!(call(&mut inv, VERSION, 1, &key, 251).is_none());
    observe(&mut inv, 2, 0x11, 1, 0x36f, &[0; 8], 252);
    assert_eq!(val(&mut inv, VERSION, 252), UNKNOWN);
    observe(&mut inv, 2, 0x11, 1, 0x2e7, &[0, 0, 0x38, 0x12, 0, 9], 253);
    assert_eq!(val(&mut inv, VERSION, 253), 1809);
}

#[test]
fn deletes_and_address_changes_do_not_retain_stale_entries() {
    let mut inv = Inventory::default();
    for n in [0, 31, 63] {
        observe(&mut inv, 0, 0, n, 0x36f, &[0; 8], 1);
    }
    assert_eq!(
        address(call(&mut inv, INVENTORY, 1, &[0, 0], 1).unwrap()),
        0
    );
    observe(&mut inv, 0, 0, 24, 0x2e7, &[0, 0, 7, 0x82, 0x83, 1], 2);
    observe(&mut inv, 0, 0, 23, 0x3e7, &[0, 24], 3);
    assert_eq!(
        address(call(&mut inv, INVENTORY, 1, &[1, 0], 3).unwrap()),
        u16::MAX
    );
    assert_eq!(
        call(&mut inv, PRODUCT, 1, &[2, 0, 0, 0, 0, 0], 3)
            .unwrap()
            .payload(),
        &[4, 0x97, 1, 0, 0, 2, 0, 0x82, 0x83, 1, 0]
    );
    assert!(call(&mut inv, PRODUCT, 0, &[2, 0, 0, 0, 0, 0], 3).is_none());
    for expected in [2, u16::MAX] {
        assert_eq!(
            address(call(&mut inv, INVENTORY, 0, &[2, 0], 3).unwrap()),
            expected
        );
    }
    observe(&mut inv, 0, 0, 23, 0x36f, &[0; 8], 4);
    assert_eq!(
        address(call(&mut inv, INVENTORY, 1, &[2, 0], 4).unwrap()),
        2
    );
    assert_eq!(
        call(&mut inv, INVENTORY, 0, &[0, 0], 4).unwrap().payload(),
        &[3, 0x67, 1, 0, 0, 0, 0]
    );
    assert_eq!(
        address(call(&mut inv, INVENTORY, 1, &[0, 0], 4).unwrap()),
        0
    );
}

#[test]
fn demand_is_not_the_configured_mode_and_read_only_updates_echo_actual_values() {
    let mut inv = Inventory::new();
    for (id, body, temp, demand) in [
        (0x36f, &[32, 3, 0, 0x35, 6, 0, 0, 0][..], 0x635, 3),
        (0x36f, &[32, 2, 0, 0, 0x40, 0, 0, 0], 0xffff, 0),
        (0x377, &[0, 0, 3, 0, 0x34, 6, 3, 0, 0, 0, 0, 0, 0], 0x634, 1),
        (
            0x377,
            &[0, 0, 1, 0, 0, 0x80, 3, 0, 0, 0, 0, 0, 0],
            0xffff,
            3,
        ),
        (0x22f, &[0, 0x1c, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0], 0xffff, 3),
        (0x22f, &[0, 0x14, 0, 0, 0x22, 6, 0, 0, 0, 0, 0], 0x622, 1),
    ] {
        observe(&mut inv, 0, 0, 24, id, body, 0);
        let r = call(&mut inv, 0x137, 0, &[1, 0, 0xaa, 0x55], 0).unwrap();
        assert_eq!(r.payload()[0], 3);
        assert_eq!(r.payload()[7..], u16::to_le_bytes(temp));
        assert_eq!(
            call(&mut inv, 0x12f, 1, &[1, 0], 0).unwrap().payload()[7],
            demand
        );
    }
    assert!(!inv.has_reports());
}

#[test]
fn floor_source_follows_observed_configuration_and_resets_on_expiration() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 24, 0x36f, &[0, 0, 0, 0x11, 6, 0, 0, 0], 1);
    let q = observe(&mut inv, 0, 0, 24, 0x26f, &[0, 0, 0x33, 6], 2).unwrap();
    assert_eq!(q[4], 13);
    let value = |inv: &mut Inventory, m, now| {
        let r = call(inv, m, 1, &[1, 0], now).unwrap();
        u16::from_le_bytes(r.payload()[7..].try_into().unwrap())
    };
    assert_eq!(value(&mut inv, 0x138, 2), 0xffff);
    for (mask, sensors, expected_room, expected_floor) in [
        (16, &[2][..], 0x633, 0x633),
        (17, &[1, 0, 0, 0, 2], 0x611, 0x633),
        (1, &[1], 0x611, 0xffff),
    ] {
        let mut b = [0; 8];
        b[0] = 13;
        b[2] = mask;
        b[3..3 + sensors.len()].copy_from_slice(sensors);
        observe(&mut inv, 0, 0, 24, 0x2e7, &b[..3 + sensors.len()], 3);
        assert_eq!(value(&mut inv, 0x137, 3), expected_room);
        assert_eq!(value(&mut inv, 0x138, 3), expected_floor);
    }
    observe(&mut inv, 0, 0, 24, 0x323, &[0, 0, 46, 0, 0, 0], 160);
    assert_eq!(value(&mut inv, 0x137, 160), 0xffff);
    assert_eq!(
        call(&mut inv, 0x150, 1, &[1, 0, 0], 160).unwrap().payload()[7],
        46
    );
    assert!(call(&mut inv, 0x150, 0, &[1, 0, 99], 160).is_none());
}

#[test]
fn reports_keep_latest_observation_per_field_with_explicit_disable_and_no_bus_writes() {
    let mut inv = Inventory::new();
    observe(&mut inv, 3, 45, 31, 0x3d7, &[5, 0x82], 0);
    assert!(!inv.has_reports());
    assert_eq!(
        call(&mut inv, 0x107, 0, &[0, 0], 0).unwrap().payload(),
        &[3, 7, 1, 0, 0, 5, 0x82]
    );
    assert_eq!(
        call(&mut inv, 0x10f, 0, &[3], 0).unwrap().payload(),
        &[3, 15, 1, 0, 0, 1]
    );
    call(&mut inv, 0x16f, 0, &[1], 0);
    for flags in [3, 5] {
        observe(&mut inv, 0, 0, 24, 0x36f, &[0, flags, 0, 1, 6, 0, 0, 0], 1);
    }
    observe(&mut inv, 3, 45, 31, 0x3d7, &[0, 0], 1);
    assert_eq!(
        inv.next_report(1).unwrap().payload(),
        &[2, 7, 1, 0, 0, 0, 0]
    );
    for (m, body) in [
        (0x1f, &[1, 0, 1, 0][..]),
        (0x37, &[1, 0, 1, 6]),
        (0x2f, &[1, 0, 0]),
        (0x77, &[1, 0, 1]),
    ] {
        let r = inv.next_report(1).unwrap();
        assert_eq!(r.payload()[..5], [2, m, 1, 0, 0]);
        assert_eq!(&r.payload()[5..], body);
    }
    assert!(!inv.has_reports());
    observe(&mut inv, 0, 0, 24, 0x36f, &[0; 8], 2);
    call(&mut inv, 0x10f, 0, &[0], 2);
    assert!(!inv.has_reports());
    call(&mut inv, 0x10f, 0, &[1], 2);
    assert!(inv.next_report(2).is_none());
    observe(&mut inv, 0, 0, 24, 0x36f, &[0; 8], 3);
    assert!(inv.next_report(154).is_none());
}

#[test]
fn attributes_follow_native_sources_not_the_requested_mask_or_product() {
    let mut inv = Inventory::new();
    let read = |inv: &mut Inventory| {
        let reply = call(inv, 0x11f, 1, &[1, 0], 0).unwrap();
        u16::from_le_bytes(reply.payload()[7..].try_into().unwrap())
    };
    assert!(call(&mut inv, 0x11f, 1, &[1, 0, 0, 0], 0).is_none());
    for (id, body, mask) in [
        (0x2e7, &[0, 0, 7, 1, 2, 3][..], 0),
        (0x36f, &[32, 0, 0, 0, 0, 0, 0, 0], 1), // Selected cool, but not operating.
        (0x36f, &[32, 1, 0, 0, 0, 0, 0, 0], 3),
        (0x2e7, &[9, 0, 0], 3),
        (0x2e7, &[9, 0, 1, 0], 11),
        (0x2e7, &[0xa3, 0, 3, 1, 1], 11), // No RH source.
        (0x323, &[0, 0, 41, 0, 0, 0], 27),
        (0x2e7, &[0xa3, 0, 3, 1, 0], 91),
        (0x2e7, &[0xa3, 0, 3, 0, 1], 59),
        (0x2e7, &[0xa3, 0, 3, 0xff, 0], 27),
        (0x26f, &[0; 4], 27), // SlabStatus without sensor configuration.
        (0x2e7, &[13, 0, 3, 1, 2], 0x11b),
        (0x2e7, &[13, 0, 1, 2], 0x11c),
        (0x377, &[0; 13], 0x11c), // No room heat/cool in a floor-only setup.
        (0x2e7, &[13, 0, 1, 1], 0x18),
        (0x377, &[0; 13], 0x1b),
        (0x22f, &[0; 11], 0x1b),
        (0x2e7, &[0x26, 0, 1, 0], 0x9b),
    ] {
        observe(&mut inv, 0, 0, 24, id, body, 0);
        assert_eq!(read(&mut inv), mask, "{id:x} {body:?}");
        assert!(!inv.has_reports());
    }
    assert_eq!(
        call(&mut inv, 0x11f, 0, &[1, 0, 255, 255], 0)
            .unwrap()
            .payload(),
        [3, 0x1f, 1, 0, 0, 1, 0, 0x9b, 0]
    );
    assert!(!inv.has_command(0));
    assert!(call(&mut inv, 0x11f, 1, &[1, 0], 151).is_none());
    observe(&mut inv, 0, 0, 24, 0x25f, &[0; 5], 151);
    assert_eq!(
        call(&mut inv, 0x11f, 1, &[1, 0], 151).unwrap().payload()[7..],
        [8, 0]
    );
}
