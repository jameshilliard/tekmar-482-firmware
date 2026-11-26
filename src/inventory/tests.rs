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
    cache.observe(port, &frame[..4 + body.len()], now, true)
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
        (0x37, &[1, 0, 1, 6][..]),
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
