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
