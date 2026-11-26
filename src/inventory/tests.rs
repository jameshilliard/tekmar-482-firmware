use super::*;
use crate::uptime::Sample;
mod read_scheduler;

#[test]
fn network_error_preserves_events_and_reads_latest_even_before_the_queue_drains() {
    let mut inv = Inventory::new();
    observe(&mut inv, 2, 9, 0, 0x3d7, &[0xc6, 0xa5], 0);
    assert!(!inv.has_reports());
    inv.set_reporting(true);
    assert!(!inv.has_reports()); // No synthetic alarm on enable.
    for (port, body) in [(1, [0x8f, 0xe1]), (4, [0x8f, 0xe1]), (1, [0, 0x80])] {
        observe(&mut inv, port, 9, 0, 0x3d7, &body, 0);
    }
    observe(&mut inv, 1, 9, 0, 0x3d7, &[0x95], 0);
    assert_eq!(
        call(&mut inv, 0x107, 1, &[], 0).unwrap().payload(),
        &[4, 7, 1, 0, 0, 0, 0x80]
    );
    for word in [[0x8f, 0xe1], [0x8f, 0xe1], [0, 0x80]] {
        assert_eq!(
            inv.next_report(0).unwrap().payload(),
            &[2, 7, 1, 0, 0, word[0], word[1]]
        );
    }
    assert!(!inv.has_reports());
    for _ in 0..NETWORK_ERROR_REPORT_CAPACITY {
        observe(&mut inv, 0, 0, 1, 0x3d7, &[0xc7, 0], 0);
    }
    observe(&mut inv, 0, 0, 1, 0x3d7, &[0, 0], 0);
    for _ in 1..NETWORK_ERROR_REPORT_CAPACITY {
        assert_eq!(inv.next_report(0).unwrap().payload()[5], 0xc7);
    }
    assert_eq!(inv.next_report(0).unwrap().payload()[5..], [0, 0]);
    assert!(!inv.has_reports());
    observe(&mut inv, 0, 0, 1, 0x3d7, &[0xc7, 0], 0);
    inv.set_reporting(false);
    assert!(!inv.has_reports());
    assert_eq!(
        call(&mut inv, 0x107, 0, &[0, 0], 0).unwrap().payload(),
        &[3, 7, 1, 0, 0, 0xc7, 0]
    );
}

fn observe(
    cache: &mut Inventory,
    port: usize,
    subnet: u8,
    node: u8,
    method: u32,
    body: &[u8],
    now: u32,
) {
    let mut frame = [0; 67];
    let h = method << 20 | u32::from(subnet) << 12 | u32::from(node) << 6 | body.len() as u32;
    frame[..4].copy_from_slice(&h.to_be_bytes());
    frame[4..4 + body.len()].copy_from_slice(body);
    cache.observe(port, &frame[..4 + body.len()], now);
}

fn probe(cache: &mut Inventory, now: u32) -> Option<[u8; 11]> {
    (0..PORTS)
        .find_map(|p| cache.next_probe(p, Sample::from_seconds(now)))
        .map(BlockProbe::body)
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
fn normal_identity_is_a_single_read_only_query_on_the_selected_line() {
    let mut inv = Inventory::new();
    assert!(!inv.line(1).identify(0x42, 0));
    assert!(!inv.line(1).identify(0x42, 32));
    assert!(inv.line(1).identify(0x42, 31));
    assert!(!inv.line(1).identify(0x42, 2));
    assert_eq!(inv.line(2).pending(), None);
    let query = inv.line(1).pending().unwrap();
    assert_eq!(
        query.frame(1),
        Request::new(1, &[0x6d, 0x70, 0, 7, 0, 0, 0x42, 0, 0, 0, 0x40])
    );
    inv.line(1).finished(query);
    assert_eq!(inv.line(1).pending(), None);
}

#[test]
fn line_discovery_is_per_port_one_shot_and_not_device_presence() {
    use crate::tn4::tx::Outcome;
    use LineProbe::{MasterQuery, NetworkInit};
    let mut inv = Inventory::new();
    let start = u32::MAX - 10;
    inv.tick(start);
    for p in 0..5 {
        assert_eq!(inv.line(p).pending(), Some(NetworkInit));
        assert_eq!(
            NetworkInit.frame(p as u8),
            Request::new(p as u8, &[0x7d, 0xf0, 0, 0])
        );
        inv.line(p).finished(NetworkInit);
        inv.line(p).sent(Outcome::NoAck);
    }
    let up = start + 1;
    inv.line(0).sent(Outcome::Ack);
    observe(&mut inv, 1, 3, 0, 0x3ff, &[1], up);
    inv.tick(up);
    for p in 0..5 {
        assert_eq!(inv.line(p).pending(), (p <= 1).then_some(MasterQuery));
        if p <= 1 {
            assert_eq!(
                MasterQuery.frame(p as u8),
                Request::new(p as u8, &[0x7f, 0x70, 0, 0])
            );
            inv.line(p).finished(MasterQuery);
            inv.line(p).sent(Outcome::Ack);
        }
    }
    observe(&mut inv, 0, 3, 0, 0x3ff, &[1], up);
    observe(&mut inv, 1, 3, 0, 0x3df, &[], up);
    inv.tick(up.wrapping_add(29));
    assert!((0..5).all(|p| inv.line(p).pending().is_none()));
    inv.tick(up.wrapping_add(30));
    assert_eq!(inv.line(0).pending(), Some(NetworkInit));
    assert!((1..5).all(|p| inv.line(p).pending().is_none()));
    // A peer probe also cancels an init still waiting for a free driver.
    observe(&mut inv, 0, 3, 0, 0x3df, &[], 30);
    inv.tick(600);
    assert!((0..5).all(|p| inv.line(p).pending().is_none()));
    assert_eq!(address(call(&mut inv, 0x167, 1, &[0, 0], 600).unwrap()), 0);
    inv.reset();
    inv.tick(601);
    assert!((0..5).all(|p| inv.line(p).pending() == Some(NetworkInit)));
}

#[test]
fn master_routes_from_its_header_without_a_hydronic_payload() {
    let mut inv = Inventory::new();
    let key = 3101u16.to_le_bytes();
    let present = |inv: &mut Inventory| address(call(inv, INVENTORY, 1, &key, 1).unwrap());
    observe(&mut inv, 3, 1, 0, 0x3ff, &[0x55], 0);
    observe(&mut inv, 3, 1, 24, 0x36f, &[0; 8], 0);
    assert_eq!(present(&mut inv), 3101);
    // Identical display subnet, different full wire address. No payload
    // from which to decode boiler or mixing fields is required to route.
    observe(&mut inv, 3, 9, 0, 0x3ff, &[], 1);
    assert_eq!(present(&mut inv), u16::MAX);
    observe(&mut inv, 3, 1, 24, 0x36f, &[0; 8], 1);
    assert_eq!(present(&mut inv), u16::MAX);
    observe(&mut inv, 3, 9, 24, 0x36f, &[0; 8], 1);
    assert_eq!(probe(&mut inv, 1).unwrap()[6], 9);
    assert_eq!(present(&mut inv), 3101);
}

#[test]
fn discovery_rearms_only_after_a_sampled_loss_and_recovery() {
    use crate::tn4::tx::Outcome;
    use LineProbe::{MasterQuery, NetworkInit};
    let mut link = NormalLine::COLD;
    link.peer(0x3df); // link-up service clears the earlier seen flag
    link.tick(1);
    assert_eq!(link.pending(), Some(MasterQuery));
    link.tick(40); // a held-off master query stays first
    link.finished(MasterQuery);
    assert_eq!(link.pending(), Some(NetworkInit));
    link.finished(NetworkInit);
    for result in [
        Outcome::NoAck,
        Outcome::ArbitrationLost,
        Outcome::Late,
        Outcome::Ack,
    ] {
        link.sent(result);
        link.tick(41);
        assert_eq!(link.pending(), None);
    }
    link.sent(Outcome::NoAck);
    link.sent(Outcome::NoAck);
    link.tick(42);
    link.peer(0x3ff);
    link.tick(43);
    assert_eq!(link.pending(), Some(MasterQuery));
    link.finished(MasterQuery);
    for _ in 0..9 {
        link.frame_error();
    }
    link.tick(44);
    link.sent(Outcome::Ack);
    link.tick(45);
    assert_eq!(link.pending(), None);
    for _ in 0..10 {
        link.frame_error();
    }
    link.tick(46);
    link.sent(Outcome::Ack);
    link.tick(47);
    assert_eq!(link.pending(), Some(MasterQuery));
    link.finished(MasterQuery);
    link.sent(Outcome::EchoMismatch);
    link.peer(0x3ff); // recovered before the service sampled down
    link.tick(48);
    assert_eq!(link.pending(), None);
}

#[test]
fn clock_update_admission_and_native_boundaries() {
    let mut inv = Inventory::new();
    inv.set_reporting(true);
    let update = |inv: &mut Inventory, b: &[u8]| {
        let reply = call(inv, 0x1a7, 0, b, 0).unwrap();
        assert_eq!(&reply.payload()[..5], &[3, 0xa7, 1, 0, 0]);
        <[u8; 7]>::try_from(&reply.payload()[5..]).unwrap()
    };
    let first = [0xd0, 7, 1, 1, 1, 0, 0];
    let last = [0xcf, 8, 12, 31, 7, 23, 59];
    assert!(call(&mut inv, 0x1a7, 1, &first, 0).is_none());
    for (i, v) in [
        (0, 0xcf),
        (1, 9),
        (2, 0),
        (2, 13),
        (3, 0),
        (3, 32),
        (4, 0),
        (4, 8),
        (5, 24),
        (6, 60),
    ] {
        let mut bad = first;
        bad[i] = v;
        assert_eq!(update(&mut inv, &bad), [255; 7]);
    }
    assert_eq!(update(&mut inv, &first[..6]), [255; 7]);
    assert_eq!(update(&mut inv, &[0; 8]), [255; 7]);
    assert_eq!(update(&mut inv, &first), first);
    let tx = inv.next_broadcast(0, 0).unwrap();
    assert_eq!(update(&mut inv, &last), [255; 7]); // other ports still queued
    for p in 1..PORTS {
        assert_eq!(
            inv.next_broadcast(p, 0).unwrap().frame(p as u8),
            Request::new(p as u8, &[0x71, 0xf0, 0, 5, 0, 1, 0, 0, 0])
        );
    }
    assert_eq!(update(&mut inv, &last), last);
    assert!(!inv.stale_broadcast(0, tx, 0)); // new clock cannot cancel a dequeued frame
    for p in 0..PORTS {
        assert_eq!(
            inv.next_broadcast(p, 0).unwrap().frame(p as u8),
            Request::new(p as u8, &[0x71, 0xf0, 0, 5, 59, 191, 11, 30, 255])
        );
        assert!(!inv.has_broadcast(p));
    }
    assert!(!inv.has_reports());
    call(&mut inv, 0x13d, 0, &[1, 1], 0);
    assert_eq!(update(&mut inv, &first), [255; 7]); // non-clock traffic also counts
}

#[test]
fn outdoor_source_precedence_expiry_and_unsent_values() {
    let mut inv = Inventory::new();
    let host = |i: &mut Inventory, value: u16, now| {
        address(call(i, 0x117, 0, &value.to_le_bytes(), now).unwrap())
    };
    assert_eq!(
        address(call(&mut inv, 0x117, 1, &[0; 2], 0).unwrap()),
        0xffff
    );
    assert!(call(&mut inv, 0x117, 0, &[3], 0).is_none());
    assert_eq!(host(&mut inv, 0xbea, 0), 0xffff);
    assert!(!inv.has_broadcast(0));
    assert_eq!(host(&mut inv, 0xbe9, 0), 0xbe9);
    assert!(!inv.has_reports());
    let queued = inv.next_broadcast(0, 0).unwrap();
    assert_eq!(
        queued.frame(0),
        Request::new(0, &[0x72, 0xf0, 0, 4, 0xe9, 0xb, 0xe9, 0xb])
    );
    observe(&mut inv, 4, 0, 0, 0x32f, &[0xe9, 0xb], 0);
    assert!(inv.stale_broadcast(0, queued, 0)); // same word, actual sensor now owns it
    inv.set_reporting(true);
    observe(&mut inv, 4, 0, 0, 0x32f, &[0x48, 6, 0, 0], 1);
    assert!(inv.stale_broadcast(0, queued, 1));
    assert!(inv.next_broadcast(1, 1).is_none()); // don't displace the actual sensor
    assert_eq!(
        inv.next_report(1).unwrap().payload(),
        &[2, 0x17, 1, 0, 0, 0x48, 6]
    );
    assert_eq!(host(&mut inv, 1200, 500), 0x648);
    inv.tick(630); // only 629 seconds since the sensor
    assert!(!inv.has_reports());
    inv.tick(631);
    assert_eq!(address(inv.next_report(631).unwrap()), 1200);
    for p in 0..5 {
        let tx = inv.next_broadcast(p, 631).unwrap();
        assert_eq!(
            tx.frame(p as u8),
            Request::new(p as u8, &[0x72, 0xf0, 0, 4, 0xb0, 4, 0xb0, 4])
        );
    }
    inv.tick(740);
    assert_eq!(address(inv.next_report(740).unwrap()), 0xffff);
    assert_eq!(address(call(&mut inv, 0x167, 1, &[0, 0], 740).unwrap()), 0);
}

#[test]
fn outdoor_request_captures_one_response_per_line_without_extending_a_sample() {
    let mut inv = Inventory::new();
    inv.set_reporting(true);
    observe(&mut inv, 4, 0, 0, 0x32f, &[0x23, 5], u32::MAX - 100);
    inv.set_reporting(false);
    assert!(!inv.has_reports());
    observe(&mut inv, 2, 1, 0, 0x327, &[7], 550);
    assert!(!inv.has_broadcast(0));
    observe(&mut inv, 2, 1, 0, 0x327, &[], 553);
    inv.tick(559); // external-only TTL: 660 s across wrap
    assert_eq!(address(call(&mut inv, 0x117, 1, &[], 559).unwrap()), 0xffff);
    for p in 0..5 {
        let tx = inv.next_broadcast(p, 559).unwrap();
        assert_eq!(
            tx.frame(p as u8),
            Request::new(p as u8, &[0x72, 0xf0, 0, 4, 0x23, 5, 0x23, 5])
        );
        assert!(!inv.has_broadcast(p));
    }
    assert!(!inv.has_reports());
}

#[test]
fn sustained_host_source_renews_on_bus_but_invalid_update_does_not_renew_lease() {
    let mut inv = Inventory::new();
    for t in [0, 200, 400] {
        inv.tick(t);
        assert_eq!(address(call(&mut inv, 0x117, 0, &[0, 0], t).unwrap()), 0);
        for p in 0..5 {
            assert_eq!(inv.next_broadcast(p, t).is_some(), t == 0);
        }
    }
    inv.tick(599);
    assert!(!inv.has_broadcast(0));
    inv.tick(600);
    assert!(inv.has_broadcast(0));
    call(&mut inv, 0x117, 0, &[255, 255], 639);
    inv.tick(640);
    assert_eq!(address(call(&mut inv, 0x117, 1, &[], 640).unwrap()), 0xffff);
}

#[test]
fn taking_address_is_report_only_and_follows_the_known_device() {
    let mut inv = Inventory::new(); // Reporting disabled.
    let move_to = |inv: &mut Inventory, subnet: u8, old: [u8; 2], now| {
        let header = 0x3e7_00000 | u32::from(subnet) << 12 | 23 << 6 | 2;
        let [a, b, c, d] = header.to_be_bytes();
        inv.observe(0, &[a, b, c, d, old[0], old[1]], now)
            .map(AddressMove::report)
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
        let call = Call {
            method: TAKING_ADDRESS,
            service,
            body: &[1, 0, 2, 0],
        };
        assert!(matches!(inv.handle(&call, 0), Answer::Unhandled));
        assert_eq!(call.unsupported().payload(), &[service + 3, 0, 0, 0, 0]);
    }
}

#[test]
fn all_stock_input_method_ids_reach_normal_handlers() {
    // The 26 cases in J1196J's type-6 dispatcher at 0xae8c. A handled
    // report-only service or an unknown device can legitimately be silent.
    for method in [
        0x107, 0x10f, 0x117, 0x11f, 0x127, 0x12f, 0x137, 0x138, 0x13d, 0x13e, 0x13f, 0x147, 0x14f,
        0x150, 0x151, 0x152, 0x157, 0x167, 0x16f, 0x177, 0x17f, 0x187, 0x18f, 0x197, 0x19f, 0x1a7,
    ] {
        let mut inv = Inventory::new();
        for service in [0, 1] {
            assert!(
                !matches!(
                    inv.handle(
                        &Call {
                            method,
                            service,
                            body: &[]
                        },
                        0
                    ),
                    Answer::Unhandled
                ),
                "missing service {service} for {method:#x}"
            );
        }
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
    assert!((0..5).all(|p| !inv.has_broadcast(p)));
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
    let first = inv.next_broadcast(0, 11).unwrap();
    assert_eq!(first.frame(0), Request::new(0, &[0x61, 0xf0, 0, 2, 1, 1]));
    assert!(!inv.stale_broadcast(0, first, 11));
    call(&mut inv, 0x13d, 0, &[1, 0], 12);
    assert!(inv.stale_broadcast(0, first, 12));
    for p in 0..5 {
        for id in (2..=12).chain([1]) {
            let frame = inv.next_broadcast(p, 13).unwrap().frame(p as u8);
            assert_eq!(
                frame,
                Request::new(p as u8, &[0x61, 0xf0, 0, 2, id, u8::from(id != 1)])
            );
        }
        assert!(!inv.has_broadcast(p));
    }
    call(&mut inv, 0x13d, 0, &[1, 1], u32::MAX - 3);
    let tx = inv.next_broadcast(2, 1).unwrap();
    assert!(!inv.stale_broadcast(2, tx, 5));
    assert!(inv.stale_broadcast(2, tx, 6));
    assert!(inv.next_broadcast(0, 6).is_none());
    assert!((0..5).all(|p| !inv.has_broadcast(p)));
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
    inv.command_sent(0, Outcome::Ack);
}

#[test]
fn contested_setting_reads_again_and_ambiguous_delivery_only_verifies() {
    for outcome in [
        Outcome::NoAck,
        Outcome::EchoMismatch,
        Outcome::BadEnd,
        Outcome::Late,
    ] {
        let mut inv = Inventory::new();
        observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], 0);
        assert!(call(&mut inv, 0x127, 0, &[24, 0, 2], 0).is_none());
        expect_block_read(&mut inv, 3);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 7, 1, 3, 9], 1);
        assert!(inv.command_frame(0).is_some());
        inv.command_sent(0, Outcome::ArbitrationLost);
        expect_block_read(&mut inv, 3);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 7, 1, 4, 10], 2);
        assert_eq!(
            inv.command_frame(0),
            Request::new(0, &[0x6d, 0xf0, 0, 7, 0, 1, 3, 0, 2, 4, 10])
        );
        inv.command_sent(0, outcome);
        expect_block_read(&mut inv, 3);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 7, 1, 4, 10], 3);
        assert!(inv.command_frame(0).is_none());
        assert_eq!(inv.command_reply(0).unwrap().payload()[7], 1);
    }
}

#[test]
fn contention_cannot_extend_a_command_or_retarget_a_different_application() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0x26, 0, 1, 5], 0);
    assert!(call(&mut inv, 0x13e, 0, &[24, 0, 0, 40, 6], 0).is_none());
    expect_block_read(&mut inv, 0x26);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0x26, 0, 1, 5], 1);
    expect_block_read(&mut inv, 0x30);
    observe(
        &mut inv,
        0,
        0,
        1,
        0x2e7,
        &[0x30, 0, 255, 1, 6, 2, 6, 3, 6, 4, 6],
        1,
    );
    assert!(inv.command_frame(0).is_some());
    inv.command_sent(0, Outcome::ArbitrationLost);
    expect_block_read(&mut inv, 0x26);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0x26, 0, 1, 2], 2);
    assert!(inv.command_frame(0).is_none());
    assert_eq!(&inv.command_reply(0).unwrap().payload()[8..], &[255, 255]);

    assert!(call(&mut inv, 0x13e, 1, &[24, 0, 0], 4).is_none());
    let req = inv.command_frame(0).unwrap();
    inv.command_sent(0, Outcome::ArbitrationLost);
    assert_eq!(inv.command_frame(0), Some(req));
    inv.command_expire(0, 14);
    inv.command_sent(0, Outcome::ArbitrationLost);
    assert!(inv.command_frame(0).is_none());
    assert_eq!(&inv.command_reply(0).unwrap().payload()[8..], &[255, 255]);
}

#[test]
fn backoff_is_per_line_idle_time_not_wall_time_or_a_replay_budget() {
    let mut inv = Inventory::new();
    let start = u32::MAX - 100;
    for _ in 0..9 {
        inv.line(0).frame_error();
        assert!(inv.line(0).may_start(start, true));
    }
    inv.line(0).frame_error();
    assert!(!inv.line(0).may_start(start, true));
    assert!(!inv.line(0).may_start(399_899, true)); // 400,000 us across wrap
    assert!(!inv.line(0).may_start(399_900, false));
    inv.line(0).peer(0x3ff);
    assert!(!inv.line(0).may_start(2_000_000, true));
    assert!(!inv.line(0).may_start(2_600_271, true));
    assert!(inv.line(0).may_start(2_600_272, true));
    inv.line(1).sent(Outcome::NoAck);
    inv.line(1).sent(Outcome::NoAck);
    assert!(inv.line(1).may_start(3_000_000, true));
    inv.line(0).start_backoff();
    inv.reset(); // No Normal cooldown carried across a mode change.
    assert!(inv.line(0).may_start(3_000_000, false));
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
        inv.command_sent(0, Outcome::Ack);
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
    observe(&mut inv, 0, 0, 1, 0x26f, &[0x81, 50, 10, 0], 1);
    assert!(!core::iter::from_fn(|| inv.next_report(1)).any(|r| r.payload()[1] == 0x4f));
}

#[test]
fn fan_uses_ventilation_block_not_duct_status_or_an_unoccupied_away_alias() {
    let mut inv = Inventory::new();
    observe(&mut inv, 0, 0, 1, 0x2e7, &[0, 0, 63, 1, 0, 0, 1, 0, 0], 0);
    observe(&mut inv, 0, 0, 1, 0x25f, &[0x33, 0, 0, 0, 0], 0);
    assert_eq!(probe(&mut inv, 0).unwrap()[4], 9);
    observe(&mut inv, 0, 0, 1, 0x25f, &[0; 5], 1);
    assert!(probe(&mut inv, 1).is_none());
    observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], 1);
    call(&mut inv, 0x16f, 0, &[1], 1);
    assert!(call(&mut inv, 0x157, 0, &[24, 0, 6, 60], 1).is_none());
    expect_block_read(&mut inv, 9);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[9, 0, 31, 10, 20, 30, 40, 50], 1);
    assert_eq!(
        inv.command_frame(0),
        Request::new(0, &[0x6d, 0xf0, 0, 9, 0, 1, 9, 0, 10, 20, 30, 40, 60])
    );
    inv.command_sent(0, Outcome::Ack);
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
fn device_targets_follow_application_and_setback_not_heat_setpoint_slots() {
    let mut inv = Inventory::new();
    let sample = |i: &mut Inventory, block: &[u8]| {
        observe(i, 0, 0, 1, 0x2e7, block, 0);
    };
    observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], 0);
    assert_eq!(
        call(&mut inv, 0x13e, 0, &[24, 0, 0, 1, 1], 0)
            .unwrap()
            .payload()[7..],
        [0, 255, 255]
    );
    assert!(!inv.has_command(0));
    observe(
        &mut inv,
        0,
        0,
        1,
        0x22f,
        &[6, 16, 0x56, 4, 0x67, 5, 0, 0, 0, 0, 0],
        0,
    );
    call(&mut inv, 0x16f, 0, &[1], 0);
    for (mode, block, selector, index) in [
        (0, 0x28, 0, 0),
        (1, 0x2a, 1, 2),
        (2, 0x2c, 2, 0),
        (3, 0x28, 3, 2),
        (4, 0x2e, 4, 0),
        (5, 0x30, 5, 2),
        (5, 0x30, 6, 4),
        (0, 0x28, 7, 2),
    ] {
        // 0x00ff is a valid 16-bit target, not the byte-sized NA sentinel.
        assert!(call(&mut inv, 0x13e, 0, &[24, 0, selector, 255, 0], 0).is_none());
        expect_block_read(&mut inv, 0x26);
        sample(&mut inv, &[0x26, 0, 1, mode]);
        expect_block_read(&mut inv, block);
        let width = if mode == 4 { 6 } else { 8 };
        let bytes = [
            block,
            0,
            if mode == 4 { 63 } else { 255 },
            10,
            1,
            20,
            2,
            30,
            3,
            40,
            4,
        ];
        let data = &bytes[..3 + width];
        sample(&mut inv, data);
        let mut frame = [
            0x6d,
            0xf0,
            0,
            4 + width as u8,
            0,
            1,
            block,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        frame[8..8 + width].copy_from_slice(&data[3..]);
        frame[8 + index..10 + index].copy_from_slice(&[255, 0]);
        assert_eq!(inv.command_frame(0), Request::new(0, &frame[..8 + width]));
        inv.command_sent(0, Outcome::Ack);
        expect_block_read(&mut inv, block);
        sample(&mut inv, data);
        assert_eq!(
            &inv.command_reply(0).unwrap().payload()[7..],
            &[
                if selector == 7 { 3 } else { selector },
                data[3 + index],
                data[4 + index]
            ]
        );
    }
}

#[test]
fn setpoint_identification_withdraws_pending_thermostat_reports_and_writes() {
    let mut inv = Inventory::new();
    inv.set_reporting(true);
    observe(&mut inv, 0, 0, 1, 0x36f, &[0; 8], 0);
    assert!(call(&mut inv, 0x127, 0, &[24, 0, 2], 0).is_none());
    expect_block_read(&mut inv, 3);
    for block in [3, 5, 6] {
        let data = if block == 3 {
            &[3, 0, 7, 1, 4, 5][..]
        } else {
            &[block, 0, 1, 48][..]
        };
        observe(&mut inv, 0, 0, 1, 0x2e7, data, 1);
    }
    // A status from another node or non-operating status isn't evidence for this device.
    let status = [0, 0x18, 21, 3, 41, 3, 33, 4, 0, 0, 0];
    observe(&mut inv, 0, 0, 2, 0x22f, &status, 1);
    observe(&mut inv, 0, 0, 1, 0x22f, &[0; 11], 1);
    assert!(inv.ports[0].job.frame_waiting());
    observe(&mut inv, 0, 0, 1, 0x22f, &status, 1);
    // Read-only unsupported replies do not contend for the outstanding native job's slot.
    for (service, body) in [(1, &[24, 0][..]), (0, &[24, 0, 1][..])] {
        assert_eq!(
            call(&mut inv, 0x127, service, body, 1).unwrap().payload(),
            &[3 + service, 0x27, 1, 0, 0, 24, 0, 255]
        );
    }
    assert!(inv.command_frame(0).is_none());
    assert_eq!(inv.command_reply(0).unwrap().payload()[7], 255);
    observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, 4], 1);
    let mut seen = 0;
    while let Some(reply) = inv.next_report(1) {
        if reply.payload()[5] != 24 {
            continue;
        }
        match (reply.payload()[1], &reply.payload()[7..]) {
            (0x38, [255, 255]) => seen |= 1,
            (0x2f, [0]) => seen |= 2,
            (0x3e, [0, 21, 3]) => seen |= 4,
            (0x27 | 0x3f | 0x47, _) => panic!("stale thermostat report"),
            _ => {}
        }
    }
    assert_eq!(seen, 7);
}

#[test]
fn setpoint_status_refreshes_addressed_setup_without_probing_on_every_report() {
    let mut inv = Inventory::new();
    let status = [0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let sample = |i: &mut Inventory, now| observe(i, 2, 9, 4, 0x22f, &status, now);
    sample(&mut inv, 0);
    let setup = [0x6d, 0x70, 0, 7, 0x26, 0, 9, 8, 0, 0, 0];
    assert_eq!(probe(&mut inv, 0), Some(setup));
    assert_eq!(probe(&mut inv, 1).unwrap()[4], 0);
    observe(&mut inv, 2, 9, 4, 0x2e7, &[0, 0, 63, 1, 1, 1, 1, 1, 1], 1);
    sample(&mut inv, 59);
    assert!(probe(&mut inv, 59).is_none());
    sample(&mut inv, 60);
    assert_eq!(probe(&mut inv, 60), Some(setup));
    observe(&mut inv, 2, 9, 4, 0x2e7, &[0x26, 0, 1, 2], 70);
    for now in [170, 280, 369] {
        sample(&mut inv, now);
        assert!(probe(&mut inv, now).is_none());
    }
    observe(&mut inv, 2, 9, 4, 0x22f, &[0; 11], 370);
    assert!(probe(&mut inv, 370).is_none());
    sample(&mut inv, 370);
    assert_eq!(probe(&mut inv, 371), Some(setup));
}

#[test]
fn device_target_reports_coalesce_with_their_own_slot_and_sentinel() {
    let mut inv = Inventory::new();
    inv.set_reporting(true);
    let status = |i: &mut Inventory, slot: u8, target: u16| {
        let [a, b] = target.to_le_bytes();
        observe(
            i,
            0,
            0,
            1,
            0x22f,
            &[slot << 1, 16, a, b, 12, 5, 0, 0, 0, 0, 0],
            0,
        );
    };
    let next = |i: &mut Inventory| {
        while let Some(reply) = i.next_report(0) {
            if reply.payload()[1] == 0x3e {
                return Some(reply);
            }
        }
        None
    };
    status(&mut inv, 0, 0x543);
    status(&mut inv, 6, 0x8000);
    observe(&mut inv, 0, 0, 1, 0x36f, &[0, 12, 0, 0, 0, 0, 0, 0], 0);
    assert_eq!(
        next(&mut inv).unwrap().payload(),
        &[2, 0x3e, 1, 0, 0, 24, 0, 6, 255, 255]
    );
    assert!(next(&mut inv).is_none());
    status(&mut inv, 1, 0x540);
    status(&mut inv, 7, 0x541);
    assert!(next(&mut inv).is_none());
}

#[test]
fn device_target_rejects_ambiguous_writes_and_reads_supported_word_pairs() {
    let mut inv = Inventory::new();
    let sample = |i: &mut Inventory, block: &[u8]| {
        observe(i, 0, 0, 1, 0x2e7, block, 1);
    };
    sample(&mut inv, &[0x26, 0, 1, 0]);
    let start = |i: &mut Inventory, service, body: &[u8]| {
        assert!(call(i, 0x13e, service, body, 1).is_none());
        expect_block_read(i, 0x26);
    };
    for body in [&[24, 0, 3, 1][..], &[24, 0, 3, 1, 2, 3]] {
        assert!(call(&mut inv, 0x13e, 0, body, 1).is_none());
        assert!(!inv.has_command(0));
    }
    assert_eq!(
        call(&mut inv, 0x13e, 0, &[24, 0, 7, 1, 2], 1)
            .unwrap()
            .payload()[8..],
        [255, 255]
    );
    for setup in [&[0x26, 0, 0, 0][..], &[0x26, 0, 1, 255]] {
        start(&mut inv, 0, &[24, 0, 3, 10, 1]);
        sample(&mut inv, setup);
        assert!(inv.command_frame(0).is_none());
        assert_eq!(inv.command_reply(0).unwrap().payload()[8..], [255, 255]);
    }
    for bad in [
        &[0x28, 0, 255, 1, 2][..],
        &[0x28, 0, 255, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        &[0x28, 0, 0x37, 1, 2, 3, 4, 5, 6, 7, 8],
        &[0x26, 0, 1, 4],
        &[0x26, 0, 1, 3], // Same table as on/off, but a different application.
    ] {
        start(&mut inv, 0, &[24, 0, 0, 10, 1]);
        sample(&mut inv, &[0x26, 0, 1, 0]);
        expect_block_read(&mut inv, 0x28);
        sample(&mut inv, bad);
        assert!(inv.command_frame(0).is_none());
        assert_eq!(inv.command_reply(0).unwrap().payload()[8..], [255, 255]);
    }
    start(&mut inv, 1, &[24, 0, 6, 0, 0]);
    sample(&mut inv, &[0x26, 0, 1, 1]);
    expect_block_read(&mut inv, 0x2a);
    sample(&mut inv, &[0x2c, 0, 0x30, 99, 99]); // wrong application's target table
    assert!(inv.command_reply(0).is_none());
    sample(&mut inv, &[0x2a, 0, 0x30, 0x45, 6]);
    assert_eq!(
        inv.command_reply(0).unwrap().payload(),
        &[4, 0x3e, 1, 0, 0, 24, 0, 6, 0x45, 6]
    );
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
        inv.command_sent(0, Outcome::Ack);
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
    inv.command_sent(0, Outcome::Ack);
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
    inv.command_sent(0, Outcome::Ack);
    assert_eq!(inv.command_frame(0), read);
    sample(&mut inv, 2);
    inv.command_sent(0, Outcome::Ack);
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
        inv.command_sent(0, Outcome::Ack);
        observe(&mut inv, 0, 0, 1, 0x2e7, &[5, 0, 31, 10, 11, 12, 13, 14], 0);
        let mut bytes = [0x6d, 0xf0, 0, 9, 0, 1, 5, 0, 0, 0, 0, 0, 0];
        bytes[8..].copy_from_slice(&want);
        assert_eq!(inv.command_frame(0), Request::new(0, &bytes));
        inv.command_sent(0, Outcome::Ack);
        assert!(inv.command_frame(0).is_some());
        inv.command_sent(0, Outcome::Ack);
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
        inv.command_sent(0, Outcome::Ack);
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
    observe(&mut inv, 2, 0x11, 33, 0x25f, &[0; 5], start);
    let q = probe(&mut inv, start).unwrap();
    assert_eq!(q, [0x6d, 0x70, 0, 7, 0, 0, 0x11, 1, 0, 0, 0]);
    assert_eq!(probe(&mut inv, start + 1).unwrap()[4], 9);
    let key = 2124u16.to_le_bytes();
    let val = |inv: &mut Inventory, m, now| {
        u32::from_le_bytes(
            call(inv, m, 1, &key, now).unwrap().payload()[7..]
                .try_into()
                .unwrap(),
        )
    };
    assert_eq!(val(&mut inv, PRODUCT, start), UNKNOWN);
    observe(&mut inv, 2, 0x11, 1, 0x2e7, &[0, 0, 7, 1, 2, 3], 4);
    assert!(probe(&mut inv, 4).is_none());
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
    observe(&mut inv, 2, 0x11, 1, 0x25f, &[0; 5], 100);
    assert_eq!(probe(&mut inv, 100).unwrap()[4], 9);
    assert!(probe(&mut inv, 101).is_none());
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
    observe(&mut inv, 0, 0, 24, 0x26f, &[0, 0, 0x33, 6], 1);
    assert_eq!(probe(&mut inv, 1).unwrap()[4], 13);
    observe(&mut inv, 0, 0, 24, 0x36f, &[0, 0, 0, 0x11, 6, 0, 0, 0], 2);
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
        (0x3f, &[1, 0, 1, 255]),
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

fn reported_air_targets(inv: &mut Inventory, now: u32) -> [Option<[u8; 2]>; 2] {
    let mut values = [None; 2];
    while let Some(report) = inv.next_report(now) {
        let body = report.payload();
        let i = match u32::from_le_bytes(body[1..5].try_into().unwrap()) {
            0x13f => 0,
            0x147 => 1,
            _ => continue,
        };
        assert!(values[i].replace(body[7..].try_into().unwrap()).is_none());
    }
    values
}

#[test]
fn master_and_member_target_selection_matches_stock_report_bodies() {
    // Rows and columns are disabled, enabled, demand only, enabled + demand.
    let selection = [[0, 2, 0, 2], [1, 0, 1, 2], [0, 2, 0, 2], [1, 1, 1, 1]];
    for group in [0, 16, 17] {
        for gate in 0..4 {
            for h in 0..4 {
                for c in 0..4 {
                    let mut inv = Inventory::new();
                    inv.set_reporting(true);
                    let master = [
                        group | gate << 5,
                        0,
                        h | 4,
                        40,
                        0,
                        0,
                        c | 8,
                        50,
                        0,
                        0,
                        0,
                        0,
                        0,
                    ];
                    observe(&mut inv, 0, 0, 1, 0x377, &master, 0);
                    let chosen = selection[h as usize][c as usize];
                    let denied = if group <= 16 { gate } else { 0 };
                    let values = [1, 2].map(|side| {
                        Some([
                            1,
                            if chosen == side && denied != side {
                                side * 10 + 30
                            } else {
                                255
                            },
                        ])
                    });
                    assert_eq!(reported_air_targets(&mut inv, 0), values, "{master:?}");
                    // A member on a different port consults that same global group.
                    for (p, side, flags) in [(1, 1, 0x21), (2, 2, 1), (3, 1, 0x41), (4, 2, 0x81)] {
                        let member = [group | ((side - 1) << 5), flags | 24, 60, 0, 0, 0, 0, 0];
                        observe(&mut inv, p, 0, 1, 0x36f, &member, 1);
                        let expected = if denied == side || flags & 0xc0 != 0 {
                            255
                        } else {
                            60
                        };
                        assert_eq!(
                            reported_air_targets(&mut inv, 1)[usize::from(side - 1)],
                            Some([6, expected])
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn air_targets_preserve_slot_aliases_sentinels_and_mode_gates() {
    let mut inv = Inventory::new();
    inv.set_reporting(true);
    call(&mut inv, 0x16f, 0, &[1], 0);
    for (id, body) in [
        (0x2e7, &[5, 0, 31, 10, 20, 30, 40, 50][..]), // Before the active setback is known.
        (0x2e7, &[6, 0, 31, 11, 21, 31, 41, 51]),
        (0x377, &[0, 0, 5, 66, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    ] {
        observe(&mut inv, 0, 0, 1, id, body, 0);
    }
    assert_eq!(
        reported_air_targets(&mut inv, 0),
        [Some([1, 66]), Some([1, 21])]
    );
    for (selector, h, c) in [
        (2, 10, 11),
        (3, 66, 21),
        (4, 10, 11),
        (5, 66, 21),
        (6, 50, 51),
        (7, 255, 255),
    ] {
        observe(
            &mut inv,
            0,
            0,
            1,
            0x36f,
            &[0, selector << 2 | 1, 0, 0, 0, 0, 0, 0],
            0,
        );
        assert_eq!(
            reported_air_targets(&mut inv, 0),
            [Some([selector, h]), Some([selector, c])]
        );
    }
    call(&mut inv, 0x16f, 0, &[0], 0);
    observe(&mut inv, 0, 0, 1, 0x36f, &[0, 1, 255, 0, 0, 0, 0, 0], 0);
    for (mode, heat, cool) in [
        (0, false, false),
        (1, true, false),
        (2, true, true),
        (3, false, true),
        (255, true, true),
    ] {
        observe(&mut inv, 0, 0, 1, 0x2e7, &[3, 0, 1, mode], 0);
        observe(&mut inv, 0, 0, 1, 0x36f, &[0, 24, 0, 0, 0, 0, 0, 0], 0);
        assert_eq!(
            reported_air_targets(&mut inv, 0),
            [heat.then_some([6, 255]), cool.then_some([6, 51])]
        );
    }
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
        (0x2e7, &[13, 0, 0x13, 0], 0x11b), // Truncated setup cannot clear a floor sensor.
        (0x2e7, &[13, 0, 1, 2], 0x11c),
        (0x2e7, &[13, 0, 0x11, 0, 0, 0], 0x11c), // Neither packed nor positional.
        (0x2e7, &[13, 0, 0x80, 0], 0x11c),       // Extension only, no known assignment.
        (0x2e7, &[13, 0, 0], 0x11c),
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
