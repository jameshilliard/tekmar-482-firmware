# /// script
# requires-python = ">=3.10"
# dependencies = ["unicorn==2.1.4"]
# ///
"""Run the release ARM image against binary UART, capture timers and five line transceivers."""

import argparse
import os
import struct
import subprocess
import sys
from collections import deque
from pathlib import Path

from unicorn import UC_ARCH_ARM, UC_HOOK_CODE, UC_HOOK_MEM_WRITE, UC_MODE_ARM, Uc
from unicorn.arm_const import UC_ARM_REG_R0

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from frames import Decoder, encode

PORTS = [(4, 0, 1, 28), (11, 1, 1, 13), (6, 0, 2, 22), (10, 1, 0, 8), (2, 0, 0, 29)]
BITS = "10001001000110000010000010000010000110100101000111000101111"
BODY = bytes.fromhex("12300001a5")
SCENE = bytes.fromhex("2ff00003 07e001")  # 483 scene 8, eight hours.
RAW_SENDS = (SCENE, bytes.fromhex("21f00002 0401"), bytes.fromhex("31f00005 1ebf100607"),
             bytes.fromhex("01000003 000102"), bytes.fromhex("20000001 01"))
RAW_READS = ((bytes.fromhex("6d70000b 100000 00008000 00000000"), bytes.fromhex("6e70060a 10007f 90020304050607")),
             (bytes.fromhex("6d70000b 380000 00008000 02000000"), bytes.fromhex("6e70060b 3800ff 7a 90020304050607")),
             (bytes.fromhex("6d70000b 090000 00008000 00000000"), bytes.fromhex("6e700608 09001f 000205070a")),
             (bytes.fromhex("17f00003 11 01 1b"), bytes.fromhex("57e250c5 19 01 1b1204")),
             (bytes.fromhex("17f00004 12 02 1922"), bytes.fromhex("57e250c8 1a 02 220006 190300")))
TELEMETRY = ((1, bytes.fromhex("7cf25fca bc07 00c0 0000 19ff f9 ca")),
             (3, bytes.fromhex("7ce25fcb bc07 00c0 0000 19ff f9 ca35")),
             (2, bytes.fromhex("76f250c8 a4 62 2a 0000 07 0080")),
             (4, bytes.fromhex("777250cd 65 88 07 00 2c06 13 ff 00c0 e4 b005")),
             (2, bytes.fromhex("65f250c5 69 1006 0080")),
             (2, bytes.fromhex("66f250c4 81 2a 0a06")),
             (1, bytes.fromhex("723250ca 02 81 27 2206 29 2a2b 23 37")),
             (2, bytes.fromhex("64f250c9 00 06 c2 19 0a 64 56 67 e3")),
             (3, bytes.fromhex("767250c5 31 19 80 e435")),
             (1, bytes.fromhex("747250c2 cf 42")),
             (1, bytes.fromhex("74f250c7 bb 1e c3 e4 35 55 60")),
             (4, bytes.fromhex("757250c5 cb 36 3f ca 35")),
             (0, bytes.fromhex("6e7250c6 030007 07 03 05")),
             (4, bytes.fromhex("6f7250c4 65 f2 ca 35")),
             (4, bytes.fromhex("6f7250c3 68 0d ff")),
             (0, bytes.fromhex("57e250cc 19 03 220006 220106 1b1704 ca")),
             (0, bytes.fromhex("57e250c8 1a 02 22ff05 190300")))

def protected_bits(body):
    data = "".join(f"{byte:08b}" for byte in body)
    crc = 0x4599
    for bit in data:
        feedback = (crc >> 14) ^ int(bit)
        crc = ((crc << 1) & 0x7fff) ^ (0x4599 if feedback else 0)
    wire = ""
    for bit in "1" + data + f"{crc:015b}":
        # Stuff only between protected bits; the delimiter never takes a stuff bit.
        if wire.endswith(("00000", "11111")):
            wire += "0" if wire[-1] == "1" else "1"
        wire += bit
    return wire


def run(path, ram, fast=False):
    cpu = Uc(UC_ARCH_ARM, UC_MODE_ARM)
    base = 0x40001000 if ram else 0
    if not ram:
        cpu.mem_map(0, 0x10000)
    cpu.mem_map(0x40000000, 0x8000)
    cpu.mem_write(0x40000200, b"\xa5" * 0xe00)
    cpu.mem_write(base, path.read_bytes())
    for addr in (0x7FFFF000, 0xE002C000, 0xE01FC000, 0xE0014000):
        cpu.mem_map(addr, 0x1000)
    cpu.mem_write(0xE01FC088, struct.pack("<I", 1 << 10))
    syms = {
        row[2]: int(row[0], 16)
        for line in subprocess.check_output(
            ["arm-none-eabi-nm", path.with_suffix("")], text=True
        ).splitlines()
        if len(row := line.split()) == 3
    }
    cpu.mem_map(0x7FD00000, 8192)
    cpu.mem_write(0x7FD00000, b"\xa5" * 8192)
    def check_power(uc, access, addr, size, value, user):
        assert struct.unpack("<I", uc.mem_read(0xE01FC0C4, 4))[0] & (1 << 31)
    cpu.hook_add(UC_HOOK_MEM_WRITE, check_power,
                 begin=syms["__bss_start"], end=syms["__bss_start"])
    main, floor, top = syms["__main_start"], syms["__main_end"], syms["__stack_top"]
    if floor > main:
        cpu.mem_write(main, b"\xa5" * (floor - main))
    cpu.mem_write(floor, b"\xa5" * (top - floor))
    incoming = deque()
    burst = deque()
    next_byte = 0
    actual = []
    timers = [{}, {}]
    button = 1 << 14
    gpio = {0: 0, 0x10: 0, 0x14: 2 | button, 0x20: 0, 0x30: 0, 0x34: 0}
    pins = 0
    clock = 0
    uptime_offset = 0
    jump_at = None
    discovery_end = None
    retry_isp_at = None
    edges = deque()
    panel_edges = deque()
    inventory_at = None
    queued_send_at = None
    queued_events = set()
    exchanges = [deque() for _ in PORTS]
    setting_exchange = exchanges[0]
    driven = [[] for _ in PORTS]
    external = [False] * len(PORTS)
    peers = set()
    peer_ack = [False] * len(PORTS)
    ack_prefixes = [set() for _ in PORTS]
    drive_start = [0] * len(PORTS)
    expected_rx_acks = set()
    received_rx_acks = set()
    raw_mode, raw_ack_mask = False, 0
    master, init = bytes.fromhex("7f700000"), bytes.fromhex("7df00000")
    line_wires = {protected_bits(body) + "0" * 11: body for body in (master, init)}
    probe_slots = {protected_bits(body) + "00" for body in (master, init)}
    line_sends = [[] for _ in PORTS]
    wire_log = deque(maxlen=12)
    clock_frame = bytes.fromhex("71f00005 1b52 05190c")
    clock_ports = set()
    conflicts, interrupted, lost = {}, {}, set()
    raw_winner = bytes.fromhex("3f700000")
    backoff_after = 0

    def contend(p, loser, winner, tag):
        a, b = protected_bits(loser), protected_bits(winner)
        index = next(i for i, pair in enumerate(zip(a, b)) if pair[0] != pair[1])
        assert index >= 2 and a[index] == "0" and b[index] == "1" and p not in conflicts
        conflicts[p] = (a[:index + 1], b, tag)

    def compete(p, drive_bits):
        conflict = conflicts.get(p)
        if not conflict or len(drive_bits) != len(conflict[0]) or "".join(drive_bits) != conflict[0]:
            return
        prefix, bits, tag = conflicts.pop(p)
        at = drive_start[p]
        # Both peers drove the common prefix. The first different bit is
        # dominant at the physical input, and the gateway must relinquish TX.
        external[p] = True
        wire = bits + "0" * 18
        combined = list(edges)
        combined.extend((at + i * 2500, p, wire[i] == "1") for i in range(len(prefix), len(wire))
                        if wire[i] != wire[i - 1])
        edges.clear()
        edges.extend(sorted(combined))
        if not raw_mode or raw_ack_mask & (1 << p):
            expected_rx_acks.add((p, at + (len(bits) + 1) * 2500))
        interrupted[p] = (prefix + "0", tag)

    def line_progress():
        nonlocal phase, jump_at, discovery_end, inventory_at
        if phase == "line-cold" and len(actual) >= 4 and all(line_sends):
            assert all([b for b, _ in tx] == [init] for tx in line_sends)
            phase = "line-up"
            contend(1, master, bytes.fromhex("7ff05001 00"), "master")
            end = inject(bytes.fromhex("7ff00001 00"))
            inject(bytes.fromhex("7ff05000"), port=1, after=end)
        elif phase == "line-up" and all(any(b == master for b, _ in tx) for tx in line_sends[:2]):
            phase = "line-wait"
            # Only the peer on port 1 suppresses the 30-second follow-up.
            jump_at = inject(bytes.fromhex("7df05000"), port=1) + 20_000
        elif phase == "line-wait" and len(line_sends[0]) == 3:
            query_at, init_at = (at for _, at in line_sends[0][1:])
            assert abs(init_at - query_at - 30_000_000) < 250_000
            phase = "line-check"
            discovery_end = clock + 1_200_000
        elif phase == "line-backoff" and len(line_sends[2]) == 2:
            body, started = line_sends[2][-1]
            assert body == master
            idle_delay = started - uptime_offset - backoff_after
            assert 800_000 < idle_delay < 1_100_000, (idle_delay, tuple(wire_log))
            phase = "capture"
            inventory_at = inject(bytes.fromhex("6e700609 00003f 828301 2301 04"))

    def level(p, high, at):
        pin, t, ch, _ = PORTS[p]
        bit = 1 << pin
        if high == bool(gpio[0x14] & bit):
            return
        gpio[0x14] ^= bit
        flags = timers[t].get(0x28, 0) >> (3 * ch) & 7
        if flags & (1 if high else 2):
            timers[t][0x2C + 4 * ch] = at
            timers[t][0] = timers[t].get(0, 0) | (1 << (4 + ch))

    def gpio_write(uc, off, size, value, data):
        nonlocal pins, phase
        if off in (0x18, 0x1C, 0):
            for p, exchange in enumerate(exchanges):
                tx = 1 << PORTS[p][3]
                drive_bits = driven[p]
                if off == 0 and value & tx and not gpio[0] & tx:
                    drive_start[p] = clock
                    drive_bits.append("1" if pins & tx else "0")
                    ack_prefixes[p].clear()
                    if not raw_mode and p in peers:
                        ack_prefixes[p].update(probe_slots)
                        if exchange:
                            ack_prefixes[p].add(protected_bits(exchange[0][0]) + "00")
                elif off in (0x18, 0x1C) and gpio[0] & tx and value & tx:
                    drive_bits.append("1" if off == 0x18 else "0")
                    peer_ack[p] = bool(ack_prefixes[p]) and "".join(drive_bits) in ack_prefixes[p]
                    compete(p, drive_bits)
                elif off == 0 and not value & tx and gpio[0] & tx:
                    if p in interrupted:
                        expected, tag = interrupted.pop(p)
                        assert "".join(drive_bits) == expected and tag not in lost
                        lost.add(tag)
                        drive_bits.clear()
                        if tag != "master":
                            exchange.popleft()  # Raw ends here; the setting write is rebuilt.
                        continue
                    if drive_bits == ["1", "0"]:
                        # Receiver drives only the link ACK slot, never its own frame.
                        slot, = (s for s in expected_rx_acks if s[0] == p and abs(s[1] - drive_start[p]) < 625)
                        assert slot not in received_rx_acks and abs(clock - slot[1] - 2500) < 625
                        received_rx_acks.add(slot)
                        drive_bits.clear()
                        continue
                    wire = "".join(drive_bits)
                    wire_log.append((drive_start[p], "tx", p, wire[:48], len(wire)))
                    if wire in line_wires:
                        assert not raw_mode, "autonomous discovery in Raw mode"
                        line_sends[p].append((line_wires[wire], drive_start[p] + uptime_offset))
                        drive_bits.clear()
                        line_progress()
                        continue
                    if not exchange:
                        assert not raw_mode and not phase.startswith("line-"), (p, phase, wire)
                        drive_bits.clear()
                        continue
                    expected, device_reply = exchange.popleft()
                    # Ten delimiter/ACK/EOF bits, then the driver's final release.
                    assert wire == protected_bits(expected) + "0" * 11, (p, phase, expected.hex(), wire)
                    drive_bits.clear()
                    if device_reply:
                        inject(device_reply)
                    if phase == "group" and not any(exchanges):
                        inject(bytes.fromhex("60f00002 0c01"), port=4)
                    if phase == "outdoor-supply" and not any(exchanges):
                        # Request a different value after the gateway heard its own TX.
                        call(0x117, bytes.fromhex("6506"), True)
                    if phase == "outdoor-refresh" and not any(exchanges):
                        inject(bytes.fromhex("72f00604 4806 0000"), port=4)
                    if phase == "outdoor-request" and not any(exchanges):
                        inject(bytes.fromhex("6f700603 840100"))
                    if phase == "clock-during-read":
                        phase = "clock"
                        for line in exchanges:
                            line.appendleft((clock_frame, None))
                        call(0x1a7, bytes(7))  # Request is silent.
                        call(0x1a7, bytes(7), True)
                        incoming.extend(bytes.fromhex("ca0c0600a7010000dc07061a020a1be435"))
                    elif expected == clock_frame:
                        assert p not in clock_ports
                        clock_ports.add(p)
                        if len(clock_ports) == 5:
                            # Only now answer the outstanding setting read on port 0.
                            contend(0, setting_exchange[0][0], bytes.fromhex("6e700606 030007 01040a"), "write")
                            inject(bytes.fromhex("6e700606 030007 010309"))
            if off == 0:
                gpio[0] = value
            elif off == 0x18:
                pins |= value
            else:
                pins &= ~value
            for p, (_, _, _, tx) in enumerate(PORTS):
                level(p, external[p] or peer_ack[p] or bool(gpio[0] & pins & 1 << tx), clock)
        elif off in (0x38, 0x3C):
            gpio[0x34] = gpio[0x34] | value if off == 0x38 else gpio[0x34] & ~value
        else:
            gpio[off] = value

    cpu.mmio_map(0x3FFFC000, 0x1000, lambda u, o, s, d: gpio[o], None, gpio_write, None)

    def timer_read(uc, off, size, t):
        nonlocal clock, inventory_at, next_byte, queued_send_at, phase, jump_at, uptime_offset, discovery_end, retry_isp_at, backoff_after
        if off == 8:
            if t == 0 and timers[t].get(4) == 1:
                clock += 40
                if retry_isp_at is not None and clock >= retry_isp_at:
                    retry_isp_at = None
                    extension(5, update=True)
                    at_line_rate()
                if jump_at is not None and clock >= jump_at:
                    jump_at = None
                    uptime_offset += 27_000_000  # PWM uptime only; preserve tN4 bit timing.
                if discovery_end is not None and clock >= discovery_end:
                    discovery_end = None
                    assert [[b for b, _ in tx] for tx in line_sends] == [
                        [init, master, init], [init, master], [init], [init], [init],
                    ]
                    phase = "line-backoff"
                    # The cold init already missed its ACK (+16). One framing
                    # error now crosses the shared threshold and starts backoff.
                    end = inject(BODY, 2, bad_crc=True)
                    end = inject(bytes.fromhex("7ff00001 00"), 2, after=end)
                    # More than a second of valid traffic: not backoff time.
                    backoff_after = inject(bytes.fromhex("7ff0003f") + bytes(63), 2, after=end)
                while burst and next_byte <= clock:
                    assert len(incoming) < 16, "LPC2146 UART RX FIFO overrun during command burst"
                    incoming.append(burst.popleft())
                    next_byte += 53 if fast else 1042  # 8N1 character time in us.
                cpu.mem_write(
                    0xE0014008, struct.pack("<I", (0xFFFF0000 + clock + uptime_offset) & 0xFFFFFFFF)
                )
                while edges and edges[0][0] <= clock:
                    at, p, high = edges.popleft()
                    external[p] = high
                    level(p, high or peer_ack[p] or bool(gpio[0] & pins & 1 << PORTS[p][3]), at)
                while panel_edges and panel_edges[0][0] <= clock:
                    _, down = panel_edges.popleft()
                    gpio[0x14] = gpio[0x14] & ~button if down else gpio[0x14] | button
                if inventory_at is not None and clock >= inventory_at:
                    inventory_at = None
                    call(0x167, b"\0\0")
                if queued_send_at is not None and clock >= queued_send_at:
                    queued_send_at = None
                    exchanges[2].append((SCENE, None))
                    send(2, SCENE)
            return clock
        return timers[t].get(off, 0)

    def timer_write(uc, off, size, value, t):
        timers[t][off] = timers[t].get(off, 0) & ~value if off == 0 else value

    for t, addr in enumerate((0xE0004000, 0xE0008000)):
        cpu.mmio_map(addr, 0x1000, timer_read, t, timer_write, t)

    def call(method, body=b"", update=False):
        incoming.extend(
            encode(6, bytes([0 if update else 1]) + struct.pack("<I", method) + body)
        )

    def extension(method, body=b"", update=False):
        call(0x544BF000 + method, body, update)

    def at_line_rate():
        nonlocal next_byte
        assert not burst
        burst.extend(incoming)
        incoming.clear()
        next_byte = clock + (53 if fast else 1042)

    def plant_inputs():
        start = clock + 30_000
        all_edges = []
        for p in range(5):
            wire = (
                BITS[:-1]
                + ("0" if BITS[-1] == "1" else "1")
                + "0" * 18
                + BITS
                + "0" * 18
            )
            for i, bit in enumerate(wire):
                if i == 0 or bit != wire[i - 1]:
                    all_edges.append((start + i * 2500, p, bit == "1"))
            if raw_ack_mask & (1 << p):
                # First frame has bad CRC. Only its valid successor may be ACKed.
                expected_rx_acks.add((p, start + (2 * len(BITS) + 19) * 2500))
        edges.extend(sorted(all_edges))

    def send(p, body=SCENE):
        incoming.extend(encode(1, bytes([1, p]) + body))

    corrupt = bytearray(encode(6, bytes.fromhex("0005f04b54")))
    corrupt[-2] ^= 1
    incoming.extend(corrupt)
    incoming.extend(bytes.fromhex("ca0706018f01000000009e35"))
    call(0x002F35CA)
    extension(1)
    extension(4)
    decoder = Decoder()
    valid = set()
    invalid = set()
    results = set()
    reads = deque(RAW_READS)
    read_events = set()
    telemetry = []
    overlap = False
    phase = "line-cold"

    def inject(body, port=0, *, after=None, bad_crc=False):
        if not raw_mode:
            peers.add(port)
        protected = protected_bits(body)
        if bad_crc:
            protected = protected[:-1] + ("0" if protected[-1] == "1" else "1")
        wire = protected + "0" * 18
        start = (clock if after is None else after) + 30_000
        wire_log.append((start, "rx", port, body.hex()))
        if not bad_crc and (not raw_mode or raw_ack_mask & (1 << port)):
            expected_rx_acks.add((port, start + (len(protected) + 1) * 2500))
        for i, bit in enumerate(wire):
            if i == 0 or bit != wire[i - 1]:
                edges.append((start + i * 2500, port, bit == "1"))
        return start + len(wire) * 2500

    def telemetry_capture():
        nonlocal phase
        phase = "telemetry"
        after = None
        for p, body in TELEMETRY:
            after = inject(body, p, after=after)

    def start_read():
        nonlocal phase
        phase = "raw-read"
        exchanges[0].append(reads[0])
        send(0, reads[0][0])
        at_line_rate()

    def read_event(event):
        nonlocal phase, queued_send_at
        assert event not in read_events
        read_events.add(event)
        if read_events == {"result", "report"}:
            read_events.clear()
            reads.popleft()
            if reads:
                start_read()
            else:
                phase = "queued-rx"
                inject(BODY, 2)
                # Admit our send while the remote peer is transmitting its header.
                queued_send_at = clock + 30_000 + 12 * 2500

    def queued_event(event):
        assert event not in queued_events
        queued_events.add(event)
        if queued_events == {BODY, SCENE, "no-peer-ack"}:
            telemetry_capture()

    def reply(kind, data):
        nonlocal phase, overlap, raw_mode, raw_ack_mask, uptime_offset, retry_isp_at
        actual.append((kind, data))
        if kind == 6:
            method = int.from_bytes(data[1:5], "little")
            if method == 0x544BF004:
                line_progress()
            elif method == 0x167 and data[5:] == bytes(2):
                assert actual[-2] == (6, bytes.fromhex("04670100000100"))
                call(0x197, bytes.fromhex("010000000000"))
                call(0x19f, bytes.fromhex("0100"))
            elif method == 0x19f:
                assert actual[-2] == (6, bytes.fromhex("0497010000010082830100"))
                assert data[5:] == struct.pack("<HI", 1, 29104)
                call(0x10f, b"\x01", True)
            elif method == 0x187 and data[0] == 4:
                assert phase == "panel-request" and data[5:] == bytes(2)
                # Neither version Update produced a NullMethod or Response:Update.
                assert actual[-2][1] == bytes.fromhex("0497010000030082830100")
                phase = "panel-button"
                gpio[0x14] &= ~button
            elif method == 0x187 and data[0] == 2:
                assert phase == "panel-button" and data[5:] == bytes(2)
                gpio[0x14] |= button
                phase = "panel-dip"
            elif method == 0x10f and data[0] == 2:
                assert phase == "panel-dip" and data[5:] == b"\x01"
                assert not gpio[0x34] & (1 << 16)  # active-low Test lamp
                phase = "panel-effective"
                call(0x10f, b"\0")
            elif method == 0x10f and phase == "panel-effective":
                assert data[0] == 4 and data[5:] == b"\0"  # host disabled push before the test
                phase = "panel-protocol"
                # Press to hold stage 1; release and press to advance to stage 2.
                panel_edges.extend((clock + dt, down) for dt, down in
                                   [(100_000, True), (200_000, False),
                                    (300_000, True), (400_000, False)])
            elif method == 0x18f and data[0] == 2:
                assert phase == "panel-protocol" and data[5:] == b"\x03\0"
                phase = "capture"
                extension(2, b"\x01", True)
            elif method == 0x10f and data[5:] == b"\x01":
                if phase == "device-target":
                    inject(bytes.fromhex("62f0060b 0610 3406 1006 0000000000"))
                elif phase == "group":
                    for exchange in exchanges:
                        exchange.append((bytes.fromhex("61f00002 0c01"), None))
                    call(0x13d,b"\x0c\x01",True)
                    call(0x13d,b"\x0c\0")
                else:
                    inject(bytes.fromhex("76f00608 2003000a06000000"))
            elif method == 0x12f and data[0] == 2 and phase == "capture":
                assert data[5:] == bytes.fromhex("010003")
                assert (6,bytes.fromhex("023701000001000a06")) in actual
                assert (6, bytes.fromhex("021f01000001000300")) in actual
                phase = "fan-attributes"
                inject(bytes.fromhex("6e700604 09000100"))
            elif method == 0x11f and phase == "fan-attributes":
                assert data[5:] == bytes.fromhex("01000b00")
                if data[0] == 2:
                    call(0x11f, bytes.fromhex("01000000"))
                elif data[0] == 4:
                    assert encode(kind, data) == bytes.fromhex("ca0906041f01000001000b003f35")
                    call(0x11f, bytes.fromhex("0100ffff"), True)
                else:
                    assert data[0] == 3
                    phase = "settings"
                    call(0x10f,b"\0",True)
            elif method == 0x10f and data[5:] == b"\0":
                if phase == "move":
                    # Published example: node 24 to 23 = tHA address 1 to 2.
                    end = inject(bytes.fromhex("7e7005c2 0018"))
                    inject(bytes.fromhex("7e700582 0017"), after=end)
                else:
                    call(0x137,b"\x01\0\xff\xff",True)
            elif method == 0x137 and data[0] == 3:
                assert data[5:] == bytes.fromhex("01000a06")
                query = bytes.fromhex("6d700007 0300 00 00008000")
                report = bytes.fromhex("6e700606 030007 010309")
                phase = "clock-during-read"
                setting_exchange.extend([
                    (query, None),
                    (bytes.fromhex("6df00007 0018 0300 020309"), None),
                    (query, bytes.fromhex("6e700606 030007 01040a")),
                    (bytes.fromhex("6df00007 0018 0300 02040a"), None),
                    (query, report),
                ])
                call(0x127, bytes.fromhex("010002"), True)
            elif method == 0x127 and data[0] == 3:
                assert not setting_exchange and data[5:] == bytes.fromhex("010001")
                query = bytes.fromhex("6d700007 0700 00 00008000")
                setting_exchange.extend([
                    (bytes.fromhex("6d700007 0d00 00 00008000"),
                     bytes.fromhex("6e700605 0d0003 0102")),
                    (query, bytes.fromhex("6e700609 07003f 404142434450")),
                    (bytes.fromhex("6df0000a 0018 0700 454545454550"), None),
                    (query, bytes.fromhex("6e700609 07003f 444444444450")),
                ])
                call(0x14f, bytes.fromhex("01000745"), True)
            elif method == 0x14f and data[0] == 3:
                assert not setting_exchange and data[5:] == bytes.fromhex("01000044")
                phase = "device-target"
                call(0x10f,b"\x01",True)
            elif method == 0x13e and data[0] == 2:
                assert data[5:] == bytes.fromhex("0100033406")
                query = bytes.fromhex("6d700007 3000 00 00008000")
                setting_exchange.extend([
                    (bytes.fromhex("6d700007 2600 00 00008000"),
                     bytes.fromhex("6e700604 260001 05")),
                    (query, bytes.fromhex("6e70060b 3000ff 3406 2305 1004 2108")),
                    (bytes.fromhex("6df0000c 0018 3000 4906 4906 4906 2108"), None),
                    (query, bytes.fromhex("6e70060b 3000ff 4506 4606 4706 2108")),
                ])
                call(0x13e, bytes.fromhex("0100074906"), True)
            elif method == 0x13e and data[0] == 3:
                assert not setting_exchange and data[5:] == bytes.fromhex("0100034606")
                phase = "group"
                call(0x10f,b"\x01",True)
            elif method == 0x13d:
                assert data[0] != 3  # Not acknowledged by the gateway.
                if data[0] == 4 and phase == "group":
                    assert data[5:] == b"\x0c\0" and any(exchanges)
                elif data[0] == 2 and phase == "group":
                    assert not any(exchanges) and data[5:] == b"\x0c\0"
                    phase = "group-active"
                    inject(bytes.fromhex("60f00002 0c10"), port=4)
                elif data[0] == 2 and phase == "group-active":
                    assert data[5:] == b"\x0c\x01"
                    call(0x13d,b"\x0c\0")
                else:
                    assert data[0] == 4 and data[5:] == b"\x0c\x01"
                    phase = "outdoor"
                    call(0x117, bytes(2))
            elif method == 0x117:
                if data[0] == 4:
                    assert encode(kind, data) == bytes.fromhex("ca07060417010000ffff2735")
                    phase = "outdoor-supply"
                    for exchange in exchanges:
                        exchange.append((bytes.fromhex("72f00004 7206 7206"), None))
                    call(0x117, bytes.fromhex("7206"), True)
                elif data[0] == 3 and data[5:] == bytes.fromhex("6506"):
                    phase = "outdoor-refresh"
                    for exchange in exchanges:
                        exchange.append((bytes.fromhex("72f00004 6506 6506"), None))
                elif data[0] == 2 and data[5:] == bytes.fromhex("4806"):
                    call(0x117, bytes.fromhex("7206"), True)
                elif data[0] == 3 and data[5:] == bytes.fromhex("4806"):
                    phase = "outdoor-request"
                    for exchange in exchanges:
                        exchange.append((bytes.fromhex("72f00004 4806 4806"), None))
                    inject(bytes.fromhex("72700000"), port=3)
                else:
                    assert data[5:] in (bytes.fromhex("7206"), bytes.fromhex("6506"))
            elif method == 0x17f and data[0] == 2:
                assert data[5:] == bytes.fromhex("010002")
                phase = "move"
                call(0x10f, b"\0", True)
            elif method == 0x15f and phase == "move":
                assert encode(kind, data) == bytes.fromhex(
                    "ca0906025f010000010002007435"
                )
                phase = "second-move"
            elif method == 0x15f:
                assert phase == "second-move" and data == bytes.fromhex("025f01000002000300")
                call(0x15f, data[5:])
                call(0x15f, data[5:], True)
                call(0x197, b"\x03\0")
            elif method == 0x197 and phase == "second-move":
                assert data[5:] == bytes.fromhex("030082830100")
                assert actual[-3:-1] == [(6, bytes([service]) + bytes(4)) for service in (4, 3)]
                phase = "panel-request"
                call(0x187, bytes(2), True)
                call(0x18f, bytes(2), True)
                call(0x187, b"\xff\xff")
            elif method == 0x544BF002 and data[5:] == bytes([1]):
                raw_mode = True
                uptime_offset += 61_000_000
                assert [pkt for k, pkt in actual if k == 6 and pkt[1:5] == bytes.fromhex("a7010000")] == [
                    bytes.fromhex("03a7010000") + b"\xff" * 7,
                    bytes.fromhex("03a7010000dc07061a020a1b"),
                ]
                assert len(clock_ports) == 5
                call(0x187, bytes(2))
                panel_edges.extend([(clock + 200_000, True), (clock + 300_000, False)])
                extension(9, bytes([1 << 2]), True)
            elif method == 0x544BF009:
                assert raw_mode and data[0] == 3 and data[5:] == bytes([1 << 2])
                raw_ack_mask = data[5]
                plant_inputs()
            elif method == 0x544BF002 and data[5:] == bytes([0]):
                raw_mode, raw_ack_mask = False, 0
                # A discovery attempt waiting for idle must not block an ISP handoff.
                external[2] = True
                level(2, True, clock)
                extension(7)
            elif method == 0x544BF007:
                _, inputs, presses, _, _, _, sweep = struct.unpack("<7I", data[5:])
                assert presses == 4 and inputs == 1 and not sweep
                assert (6, bytes.fromhex("0406f04b54 87010000 03")) in actual
                phase = "enter-update"
                for p in range(5):
                    extension(3, bytes([p]))
                extension(5, update=True)
                # Deliver these contiguous commands at line rate into a
                # finite hardware FIFO, not the harness's unlimited backlog.
                at_line_rate()
            elif method == 0x544BF006 and data[5:] == bytes.fromhex("05f04b54 01"):
                # A Normal frame already on the wire must finish before entering ISP.
                assert phase == "enter-update" and retry_isp_at is None
                retry_isp_at = clock + 25_000
            return
        service, p = data[:2]
        if service == 4:
            assert data[6] == 2 and int.from_bytes(data[7:9], "little") == 56
            protected = int.from_bytes(data[9:], "big")
            assert (
                protected >> 55 == 1
                and ((protected >> 15) & ((1 << 40) - 1)).to_bytes(5, "big") == BODY
            )
            assert protected & 0x7FFF == 0x0E2E
            invalid.add(p)
        elif service == 0:
            if phase == "queued-rx":
                assert p == 2 and data[6:] in (BODY, SCENE)
                queued_event(data[6:])
                return
            if phase == "telemetry":
                assert (p, data[6:]) == TELEMETRY[len(telemetry)]
                telemetry.append((p, data[6:]))
                if len(telemetry) == len(TELEMETRY):
                    extension(2, bytes(1), True)
                    at_line_rate()
                return
            if phase == "raw-read":
                assert data[6:] in (RAW_SENDS[p], *reads[0])
                if data[6:] == reads[0][1]:
                    assert p == 0
                    read_event("report")
                return
            assert data[6:] == (BODY if phase == "capture" else raw_winner if p == 1 else RAW_SENDS[p])
            if phase == "capture":
                assert p in invalid
                valid.add(p)
                if len(valid) == 5:
                    phase = "sending"
                    contend(1, RAW_SENDS[1], raw_winner, "raw")
                    for n, body in enumerate(RAW_SENDS):
                        exchanges[n].append((body, None))
                        send(n, body)
                    send(0)
                    extension(2, b"\x00", True)
                    at_line_rate()
        elif service == 3:
            assert data == bytes([3, 0, 1])
            overlap = True
        elif service == 2:
            assert data[2:] == bytes([2, 2 if phase == "sending" and p == 1 else 1]), data
            if phase == "queued-rx":
                assert p == 2
                queued_event("no-peer-ack")
                return
            if phase == "raw-read":
                assert p == 0
                read_event("result")
                return
            assert p not in results and (p != 0 or overlap)
            results.add(p)
            if len(results) == 5:
                assert not any(exchanges)
                start_read()
        else:
            raise AssertionError(data)

    queued = list(incoming)
    if fast:
        incoming.clear()
    training = deque([10_000, 18_000, 90_000, 98_000, 170_000, 250_000] if fast else [])
    acr = 0
    dlab = False
    dll = dlm = draining = 0
    entered = False

    def read(uc, off, size, user):
        nonlocal draining, acr, dll, dlm
        if training and clock >= training[0]:
            at = training.popleft()
            if at < 170_000:
                gpio[0x14] ^= 2  # Wake burst on RXD0, no byte in the startup-rate FIFO.
            elif at == 170_000:
                assert acr == 3
                acr, dll, dlm = 0, 33, 0  # 2 bit periods at 187500, PCLK/16
            else:
                incoming.extend(queued)
        if off == 0x20:
            return acr
        if dlab and off in (0, 4):
            return dll if off == 0 else dlm
        if off == 0x14:
            draining = max(0, draining - 1)
            return (
                (0 if draining > 1 else 0x20)
                | (0 if draining else 0x40)
                | bool(incoming)
            )
        assert off == 0 and not dlab
        return incoming.popleft()

    def write(uc, off, size, value, user):
        nonlocal dlab, dll, dlm, draining, acr
        if off == 0x0C:
            dlab = bool(value & 128)
        elif off == 0x20:
            acr = value & 3
        elif off == 0 and dlab:
            dll = value
        elif off == 4 and dlab:
            dlm = value
        elif off == 0:
            assert dll + 256 * dlm == (16 if fast else 313)
            draining = 3
            for packet in decoder.feed([value]):
                reply(*packet)

    def rom(uc, addr, size, user):
        nonlocal entered
        assert not draining and not incoming and not burst
        assert struct.unpack("<I", uc.mem_read(0xE01FC0C4, 4))[0] & (1 << 31)
        assert uc.mem_read(uc.reg_read(UC_ARM_REG_R0), 4) == struct.pack("<I", 57)
        assert uc.mem_read(0xE01FC080, 4) == bytes(4)
        assert not gpio[0] and not gpio[0x20]
        assert all(not t[4] and not t[0x28] for t in timers)
        entered = True
        uc.emu_stop()

    cpu.mmio_map(0xE000C000, 0x1000, read, None, write, None)
    cpu.hook_add(UC_HOOK_CODE, rom, begin=0x7FFFFFF0, end=0x7FFFFFF0)
    # The scenario runs real ARM instructions while each synthetic tN4 bit elapses.
    cpu.emu_start(base, 0xFFFFFFFF, count=520_000_000)
    assert entered, (clock, phase, [len(x) for x in exchanges], results, actual[-10:],
                     line_sends, expected_rx_acks - received_rx_acks, list(wire_log))
    assert actual[:4] == [
        (6, bytes.fromhex(x))
        for x in ("048f0100000300", "0400000000", "0401f04b5401000503")
    ] + [
        (
            6,
            bytes.fromhex("0404f04b54")
            + os.getenv("TEKMAR_BUILD_REVISION", "unversioned").encode(),
        )
    ]
    assert (6, bytes.fromhex("0306f04b5402f04b5401")) in actual
    for p in range(5):
        assert (6, bytes.fromhex("0403f04b54") + bytes([p]) + bytes(8)) in actual
    assert not reads and not read_events and actual[-1] == (6, bytes.fromhex("0305f04b54"))
    assert tuple(telemetry) == TELEMETRY
    assert [b for b, _ in line_sends[2]] == [init, master]
    assert not conflicts and not interrupted and lost == {"master", "write", "raw"}
    assert (1, raw_winner) in [(b[1], b[6:]) for k, b in actual if k == 1 and b[0] == 0]
    assert expected_rx_acks and received_rx_acks == expected_rx_acks, expected_rx_acks - received_rx_acks
    painted = bytes(cpu.mem_read(floor, top - floor))
    free = len(painted) - len(painted.lstrip(b"\xa5"))
    assert free >= 256, free
    print(
        f"ARM TPCK: five-port discovery, arbitration/backoff, RX/ACK, TX/overlap, ISP; untouched stack {free} bytes"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--fast", action="store_true")
    parser.add_argument("--ram", action="store_true")
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    run(args.image, args.ram, args.fast)
