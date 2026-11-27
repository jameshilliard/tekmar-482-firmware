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
TRACE_BURST = 2 * len(PORTS)
BITS = "10001001000110000010000010000010000110100101000111000101111"
BODY = bytes.fromhex("12300001a5")
EDIT_KEY = bytes.fromhex("030018 0300 0000 07 03 010409 000409")
GROUP_KEY = bytes.fromhex("0300 01008000 3800 0703")
GROUP_READ = bytes.fromhex("6d70000b 3800 00 01008000 03000000")
EDIT_READ = bytes.fromhex("6d70000b 030000 00008000 00000000")
EDIT_WRITE = bytes.fromhex("6df00007 0018 0300 000409")
SCENE = bytes.fromhex("2ff00003 07e001")  # 483 scene 8, eight hours.
RAW_SENDS = (SCENE, bytes.fromhex("21f00002 0401"), bytes.fromhex("31f00005 1ebf100607"),
             bytes.fromhex("01000003 000102"), bytes.fromhex("20000001 01"))
RAW_READS = ((bytes.fromhex("6d70000b 100000 00008000 00000000"), bytes.fromhex("6e70060a 10007f 90020304050607")),
             (bytes.fromhex("6d70000b 380000 00008000 02000000"), bytes.fromhex("6e70060b 3800ff 7a 90020304050607")),
             (bytes.fromhex("6d70000b 090000 00008000 00000000"), bytes.fromhex("6e700608 09001f 000205070a")),
             (bytes.fromhex("17f00003 11 01 1b"), bytes.fromhex("57e250c5 19 01 1b1204")),
             (bytes.fromhex("17f00004 12 02 1922"), bytes.fromhex("57e250c8 1a 02 220006 190300")),
             (bytes.fromhex("1f600002 43a5"), bytes.fromhex("5f5250ce 43a5e7 01 410042caff00 08 3412 35")),
             (bytes.fromhex("1f400003 43a5e7"), bytes.fromhex("5f3250c6 43a5e7 08 7856")),
             (bytes.fromhex("35f00005 abf5000080"), bytes.fromhex("767ab603 ab0000")),
             (bytes.fromhex("6d70000b 260000 00008000 00000000"), bytes.fromhex("6e700605 260041 0408")),
             (bytes.fromhex("6d70000b 2e0000 00008000 00000000"), bytes.fromhex("6e700609 2e003f 3506 ca05 e404")))
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
             (0, bytes.fromhex("57e250c8 1a 02 22ff05 190300")),
             (2, bytes.fromhex("5f2250c7 43a5e7 08 ca35 e4")),
             (1, bytes.fromhex("410250c4 07 02 99 ca")),
             (1, bytes.fromhex("600250c1 03")),
             (1, bytes.fromhex("600250c2 f2 35")),
             (4, bytes.fromhex("60f250c3 04 10 e4")),
             (2, bytes.fromhex("6ef250c1 82")),
             (3, bytes.fromhex("6ff250c4 05 b400 ca")),
             (3, bytes.fromhex("70f250c2 91 35")),
             (0, bytes.fromhex("71f250c7 2d 63 ff ff ff 04 e4")),
             (4, bytes.fromhex("72f250c5 3205 3305 ca")),
             (4, bytes.fromhex("62f250cc bd d4 3500 ca00 00c0 e4 35 ca e4")))

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


def run(path, ram, fast=False, *, normal_reads=False):
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
    connect_at = None
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
    expected_flags, observed_flags = set(), set()
    ack_seen = [False] * len(PORTS)
    raw_mode, raw_ack_mask = False, 0
    master, init = bytes.fromhex("7f700000"), bytes.fromhex("7df00000")
    line_wires = {protected_bits(body) + "0" * tail: body for body in (master, init) for tail in (3, 11)}
    probe_slots = {protected_bits(body) + "00" for body in (master, init)}
    line_sends = [[] for _ in PORTS]
    wire_log = deque(maxlen=12)
    clock_frame = bytes.fromhex("71f00005 1b52 05190c")
    clock_ports = set()
    mode_services = []
    fault_words = []
    traces = []
    read_traces = []
    burst_traces = []
    congestion_reports = []
    trace_resume = 0
    trace_check_at = None
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
        nonlocal phase, jump_at, discovery_end, inventory_at, connect_at
        if phase == "line-cold" and all(line_sends) and len(observed_flags) == 5:
            assert all([b for b, _ in tx] == [init] for tx in line_sends)
            if normal_reads:
                # No UART input, subscriptions or trace while these frames arrive.
                assert not actual and not incoming and not training
                phase = "offline-traffic"
                end = inject(bytes.fromhex("123420c1 a5"), 1)
                connect_at = inject(bytes.fromhex("12342101 a5"), 1, after=end, bad_crc=True)
                return
            if len(actual) < 4:
                return
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
            phase = "cycle-read"
            # A water-cycle announcement on port 1 is not a lock on port 3.
            end = inject(bytes.fromhex("77f05001 81"), port=1)
            end = inject(bytes.fromhex("6e700609 00003f 010000 0100 00"), port=3, after=end)
            exchanges[3].append((bytes.fromhex("6d700007 0900 00 00008000"), None))
            inject(bytes.fromhex("65f00605 0000000000"), port=3, after=end)

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
        nonlocal pins, phase, inventory_at, jump_at, trace_resume
        if off in (0x18, 0x1C, 0):
            for p, exchange in enumerate(exchanges):
                tx = 1 << PORTS[p][3]
                drive_bits = driven[p]
                if off == 0 and value & tx and not gpio[0] & tx:
                    drive_start[p] = clock
                    ack_seen[p] = False
                    drive_bits.append("1" if pins & tx else "0")
                    if phase == "await-isp-flag" and p == 2:
                        assert pins & tx and any(port == p and abs(at - clock) < 625 for port, at in expected_flags)
                        phase = "isp-during-flag"
                        extension(5, update=True)
                        at_line_rate()
                    ack_prefixes[p].clear()
                    if not raw_mode and p in peers:
                        ack_prefixes[p].update(probe_slots)
                        if exchange:
                            ack_prefixes[p].add(protected_bits(exchange[0][0]) + "00")
                elif off in (0x18, 0x1C) and gpio[0] & tx and value & tx:
                    drive_bits.append("1" if off == 0x18 else "0")
                    peer_ack[p] = bool(ack_prefixes[p]) and "".join(drive_bits) in ack_prefixes[p]
                    ack_seen[p] |= peer_ack[p]
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
                        if clock - drive_start[p] > 5000:
                            assert not raw_mode, "Raw mode drove a recovery flag"
                            flags = [f for f in expected_flags if f[0] == p and abs(f[1] - drive_start[p]) < 625]
                            assert len(flags) == 1, (phase, p, drive_start[p], expected_flags - observed_flags, list(wire_log))
                            flag, = flags
                            assert flag not in observed_flags and abs(clock - flag[1] - 15000) < 625
                            observed_flags.add(flag)
                            drive_bits.clear()
                            line_progress()
                            continue
                        slot, = (s for s in expected_rx_acks if s[0] == p and abs(s[1] - drive_start[p]) < 625)
                        assert slot not in received_rx_acks and abs(clock - slot[1] - 2500) < 625
                        received_rx_acks.add(slot)
                        drive_bits.clear()
                        continue
                    wire = "".join(drive_bits)
                    # A Normal sender without a peer ends at the ACK sample.
                    tail = 3 if not raw_mode and not ack_seen[p] else 11
                    if tail == 3:
                        expected_flags.add((p, drive_start[p] + (len(wire) - 1) * 2500))
                    wire_log.append((drive_start[p], "tx", p, wire[:48], len(wire)))
                    if wire in line_wires:
                        assert not raw_mode, "autonomous discovery in Raw mode"
                        assert wire == protected_bits(line_wires[wire]) + "0" * tail
                        line_sends[p].append((line_wires[wire], drive_start[p] + uptime_offset))
                        drive_bits.clear()
                        line_progress()
                        continue
                    if not exchange:
                        assert wire != protected_bits(GROUP_READ) + "0" * tail, "duplicate group read"
                        assert wire != protected_bits(EDIT_WRITE) + "0" * tail, "unexpected duplicate or stale edit"
                        assert wire != protected_bits(SCENE) + "0" * tail, "unrequested scene copy"
                        assert not raw_mode and not phase.startswith("line-") and phase != "cycle-read", (p, phase, wire)
                        drive_bits.clear()
                        continue
                    expected, device_reply = exchange.popleft()
                    # Ten delimiter/ACK/EOF bits, then the driver's final release.
                    assert wire == protected_bits(expected) + "0" * tail, (p, phase, expected.hex(), wire)
                    drive_bits.clear()
                    if phase == "cycle-read":
                        assert p == 3 and ack_seen[p] and not exchange
                        phase = "capture"
                        end = inject(bytes.fromhex("7ff09000"), port=3)  # retire this synthetic subnet
                        inventory_at = inject(bytes.fromhex("6e700609 00003f 828301 2301 04"), after=end)
                    elif phase == "normal-block":
                        assert p == 3 and ack_seen[p] and not exchange
                        end = inject(bytes.fromhex("6e700607 380003 f3 22 ca35"), port=p)
                        end = inject(bytes.fromhex("6e700e07 380003 f3 53 ca35"), port=p, after=end)
                        for _ in range(TRACE_BURST - 1):
                            end = inject(bytes.fromhex("6e700087 380003 f3 99 ca35"), port=p, after=end)
                        end = inject(bytes.fromhex("6e700047 380003 f3 11 ca35"), port=p, after=end)
                        # Hold UART TX through the last peer. Its dropped trace
                        # must be accounted for before the terminal tRPC report.
                        trace_resume = end + 40_000
                    elif phase == "normal-status":
                        assert p == 3 and ack_seen[p] and not exchange
                        end = inject(bytes.fromhex("17e00045 090108 0102"), port=p)
                        end = inject(bytes.fromhex("17e00605 090108 35ca"), port=p, after=end)
                        inject(bytes.fromhex("17e00e06 0901ff 0405 7e"), port=p, after=end)
                    elif phase in ("edit-guard", "edit-stale") and expected == EDIT_READ:
                        other = 9 if phase == "edit-guard" else 10
                        inject(bytes.fromhex("6e700606 0300 07 0104") + bytes([other]), port=p)
                    elif phase == "normal-silent":
                        assert p == 4 and ack_seen[p] and not exchange
                        jump_at = clock + 50_000
                    if device_reply:
                        end = inject(device_reply)
                        if phase in ("device-target", "setpoint-mode", "await-setpoint-setup"):
                            # Separate the background setup probe from the host's later transaction.
                            assert expected == bytes.fromhex("6d700007 2600 00 00008000")
                            inject(bytes.fromhex("62f0060b 0610 3506 1006 0000000000"), after=end)
                    if phase == "group" and not any(exchanges):
                        inject(bytes.fromhex("60f00002 0c01"), port=4)
                    if phase == "outdoor-supply" and not any(exchanges):
                        # Request a different value after the gateway heard its own TX.
                        call(0x117, bytes.fromhex("6506"), True)
                    if phase == "outdoor-refresh" and not any(exchanges):
                        inject(bytes.fromhex("72f00604 4806 0000"), port=4)
                    if phase == "outdoor-request" and not any(exchanges):
                        phase = "setback-before-status"
                        call(0x17f, bytes.fromhex("010000"))
                    if phase == "clock-during-read":
                        phase = "clock"
                        for line in exchanges:
                            line.appendleft((clock_frame, None))
                        # Two independent readers join the pending mode write's peer readback.
                        call(0x127, bytes.fromhex("0100"))
                        call(0x127, bytes.fromhex("01000000"))
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
        nonlocal trace_check_at, connect_at
        nonlocal clock, inventory_at, next_byte, queued_send_at, phase, jump_at, uptime_offset, discovery_end, retry_isp_at, backoff_after
        if off == 8:
            if t == 0 and timers[t].get(4) == 1:
                clock += 40
                if clock > 55_000_000:
                    raise AssertionError(("emulated bus scenario timed out", phase, actual[-4:]))
                if connect_at is not None and clock >= connect_at:
                    connect_at = None
                    phase = "boot-counters"
                    connect_host()
                if trace_check_at is not None and clock >= trace_check_at:
                    trace_check_at = None
                    if phase == "trace-burst":
                        extension(11, b"\0", True)
                    else:
                        call(0x107, bytes(2))
                        extension(11)
                if retry_isp_at is not None and clock >= retry_isp_at:
                    retry_isp_at = None
                    if phase == "isp-after-flag":
                        phase = "enter-update"
                        extension(10)
                        extension(14)
                        extension(14, b"\0")
                        extension(14, bytes.fromhex("000018"))
                        extension(14, bytes.fromhex("00ab18"))
                        for p in range(5):
                            extension(3, bytes([p]))
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
    if normal_reads:
        extension(14, b"\x01")
        extension(14, bytes.fromhex("014203"))
        extension(14, bytes.fromhex("014204"))
    else:
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
    announced = {}
    telemetry = []
    error_reports = []
    error_reads = []
    stage_counts = []
    boot_port_checked = False
    boot_paths = {}
    error_masks = []
    uart_rx_fault = 0
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
        if bad_crc and not raw_mode:
            expected_flags.add((port, start + len(protected) * 2500))
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
        if event == "report":
            assert "result" in read_events, ("peer reply overtook TxResult", reads[0])
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
        if event == "no-peer-ack":
            assert BODY in queued_events, "Rx from before bus acquisition must precede TxResult"
        queued_events.add(event)
        if queued_events == {BODY, SCENE, "no-peer-ack"}:
            telemetry_capture()

    def reply(kind, data):
        nonlocal uart_rx_fault, boot_port_checked
        nonlocal trace_resume, trace_check_at
        nonlocal phase, overlap, raw_mode, raw_ack_mask, uptime_offset, retry_isp_at
        if kind == 0xd0:
            if phase == "trace-burst":
                assert data[0] == 0, ("unexpected TX, invalid frame or trace loss", data.hex())
                burst_traces.append((data[1], data[7:]))
                return
            if normal_reads:
                assert data[1] == (8 if data[0] == 5 else 3)
                read_traces.append(data)
                return
            if data[0] == 5:
                assert phase == "trace-congested" and data == bytes.fromhex("0501 0200000000000000")
                assert [r[-2] for k, r in traces if k == 0] == list(range(1, TRACE_BURST + 1))
                traces.append((data[0], data[1:]))
                return
            assert data[1] == 0 and int.from_bytes(data[2:6], "little") and not raw_mode
            traces.append((data[0], data[6:]))
            if phase == "trace-invalid":
                assert data[0] == 4 and data[6:9] == bytes.fromhex("02 3800")
                phase = "trace-congested"
                end = clock + 100_000
                for code in range(1, TRACE_BURST + 3):
                    end = inject(bytes.fromhex("7d700602") + bytes([code, 0]), after=end)
                trace_check_at, trace_resume = end + 10_000, end + 40_000
            return
        actual.append((kind, data))
        if kind == 6:
            method = int.from_bytes(data[1:5], "little")
            if method == 0x544BF004:
                line_progress()
                if not normal_reads:
                    extension(12)
                    extension(12, bytes.fromhex("0584"))
                    extension(12, b"\x24", True)  # port 2 and gateway
            elif method == 0x544bf00f:
                if phase == "normal-silent":
                    if data[0] == 4:
                        assert data[5:] == bytes.fromhex("0400 01000000 3412 0000")
                    else:
                        assert data[0] == 2 and data[5:] == bytes([4,1,0])
                        phase = "trace-burst-arm"
                        extension(11, b"\x1f", True)
                elif data[0] == 4:
                    assert phase == "normal-block"
                    assert data[5:] == GROUP_KEY
                    extension(15, GROUP_KEY)  # busy, not a second query
                    extension(2, b"\x01", True)  # changing mode cannot abandon the read
                elif data[5:7] == bytes([3,4]):
                    assert phase == "normal-block" and data[0] == 2 and data[7] == 0
                    assert data[12:] == bytes.fromhex("6e700607 380003 f3 22 ca35")
                else:
                    assert phase == "normal-block" and data[0] == 2 and data[5:7] == bytes([3,0])
                    assert data[7] == 0 and int.from_bytes(data[8:12], "little") > 0
                    assert data[12:] == bytes.fromhex("6e700047 380003 f3 11 ca35")
                    captured = [t[7:] for t in read_traces if t[0] == 0]
                    lost = sum(int.from_bytes(t[2:], "little") for t in read_traces if t[0] == 5)
                    assert captured and data[12:] not in captured and lost > 0
                    assert len(captured) + lost >= TRACE_BURST + 2
                    parts = [d for k, d in actual if k == 6 and d[:7] == bytes.fromhex("020ff04b54 0304")]
                    assert len(parts) == 1 and 0 < int.from_bytes(parts[0][8:12], "little") < int.from_bytes(data[8:12], "little")
                    assert (6, bytes.fromhex("0406f04b54 0ff04b54 01")) in actual
                    assert (6, bytes.fromhex("0306f04b54 02f04b54 01")) in actual, (actual[-10:], list(wire_log))
                    phase = "normal-status"
                    exchanges[3].append((bytes.fromhex("17f00004 010208ff"), None))
                    extension(16, bytes.fromhex("03010018 7f01 010208ff"))
            elif method == 0x544bf010:
                assert phase == "normal-status"
                if data[0] == 4:
                    assert data[5:] == bytes.fromhex("03010018 7f01 010208ff")
                    extension(15, bytes.fromhex("0300 00008000 0000 0000"))
                elif data[5:7] == bytes([3,0]):
                    assert data[0] == 2
                else:
                    assert data[0] == 2 and data[5:] == bytes([3,1,0])
                    parts = [d for k, d in actual if k == 6 and d[:7] == bytes.fromhex("0210f04b54 0300")]
                    assert [d[11:] for d in parts] == [bytes.fromhex(s) for s in ("17e00605 090108 35ca","17e00e06 0901ff 0405 7e")]
                    for part in parts:
                        assert any(t[0] == 0 and t[2:6] == part[7:11] and t[7:] == part[11:] for t in read_traces)
                    assert 0 < int.from_bytes(parts[0][7:11],"little") < int.from_bytes(parts[1][7:11],"little")
                    assert actual.count((6, bytes.fromhex("0406f04b54 0ff04b54 01"))) == 2
                    phase = "edit-guard"
                    extension(11, b"\x00", True)
                    exchanges[3].extend(((EDIT_READ, None), (EDIT_WRITE, None)))
                    extension(17, EDIT_KEY)  # Request must never mutate.
                    extension(17, EDIT_KEY, True)
            elif method == 0x544bf011:
                assert phase in ("edit-guard", "edit-stale")
                if data[0] == 3:
                    assert data[5:] == EDIT_KEY
                    extension(17, EDIT_KEY, True)  # An overlapping edit cannot replace it.
                elif phase == "edit-guard":
                    assert data[0] == 2 and data[5:] == bytes([3,0,0,0])
                    assert (6, bytes.fromhex("0406f04b54 11f04b54 02")) in actual
                    assert (6, bytes.fromhex("0306f04b54 11f04b54 01")) in actual
                    phase = "edit-stale"
                    exchanges[3].append((EDIT_READ, None))
                    extension(17, EDIT_KEY, True)
                else:
                    assert data[0] == 2 and data[5:] == bytes([3,1,0,255])
                    phase = "normal-announce"
                    peers.discard(2)
                    extension(18, bytes.fromhex("03 ff02 07e001"))  # Request cannot send.
                    for port in (2, 3):
                        exchanges[port].append((SCENE, None))
                        extension(18, bytes([port]) + bytes.fromhex("ff02 07e001"), True)
            elif method == 0x544bf012:
                assert phase == "normal-announce"
                port = data[5]
                if data[0] == 3:
                    assert data[6:] == bytes.fromhex("ff02 07e001")
                    extension(18, data[5:], True)  # No replacement or duplicate.
                else:
                    assert data[0] == 2 and len(data) == 7 and port not in announced
                    announced[port] = data[6]
                if len(announced) == 2:
                    assert announced == {2: 1, 3: 0}  # no ACK vs. ACK
                    assert actual.count((6, bytes.fromhex("0306f04b54 12f04b54 01"))) == 2
                    assert (6, bytes.fromhex("0406f04b54 12f04b54 02")) in actual
                    phase = "normal-discover"
                    exchanges[3].append((bytes.fromhex("6d70000b 0000 15 ffffff7f 00000000"), None))
                    extension(19, bytes.fromhex("0315"), True)  # Only Request may probe.
                    extension(19, bytes.fromhex("031500"))
                    extension(19, bytes.fromhex("0315"))
            elif method == 0x544bf013:
                assert phase == "normal-discover" and data[5] == 3
                if data[0] == 4:
                    assert data[6:] == b"\x15"
                    extension(19, bytes.fromhex("0316"))  # Never replace an admitted probe.
                else:
                    assert data[0] == 2 and data[6:] == b"\0"
                    assert (6, bytes.fromhex("0306f04b54 13f04b54 02")) in actual
                    assert (6, bytes.fromhex("0406f04b54 13f04b54 02")) in actual
                    assert (6, bytes.fromhex("0406f04b54 13f04b54 01")) in actual
                    phase = "normal-silent"
                    exchanges[4].append((bytes.fromhex("6d70000b 3412 00 01000000 00000000"), None))
                    extension(15, bytes.fromhex("0400 01000000 3412 0000"))
            elif method == 0x544bf00e:
                body=data[5:]
                assert data[0]==4
                if phase == "boot-counters":
                    if len(body) == 57:
                        p, rx, ack, no_ack, failed, invalid_rx, untracked, unattributed = struct.unpack("<B7Q", body)
                        assert (p, rx, invalid_rx, untracked) == (1, 1, 1, 0)
                        assert no_ack >= 1 and unattributed == ack + no_ack + failed
                        boot_port_checked = True
                    else:
                        assert len(body) == 36 and body[:2] == bytes.fromhex("0142")
                        boot_paths[body[2]] = (body[3], *struct.unpack_from("<4Q", body, 4))
                        if len(boot_paths) == 2:
                            # The CRC failure belongs to the port, never to a guessed peer.
                            assert boot_paths == {3: (1, 1, 0, 0, 0), 4: (0, 0, 0, 0, 0)}
                            phase = "normal-block"
                            peers.update((3,4))
                            extension(11, b"\x08", True)
                            exchanges[3].append((GROUP_READ, None))
                            extension(15, GROUP_KEY)
                elif len(body)==4:
                    capacity,used=struct.unpack("<HH",body)
                    assert 0<used<=capacity
                elif len(body)==57:
                    port,rx,acked,no_ack,failed,invalid_count,_,_=struct.unpack("<B7Q",body)
                    assert port==0 and rx and acked and invalid_count
                elif body[:4] == bytes.fromhex("00ab1801"):
                    stage_counts.append(struct.unpack_from("<4Q", body, 4))
                else:
                    assert len(body)==36 and body[:4]==bytes.fromhex("00001801")
                    assert sum(struct.unpack_from("<4Q",body,4))>0
                actual.pop()
            elif method == 0x544bf00c:
                if data[0] == 2:
                    source, code, ms, count = struct.unpack("<BBIQ", data[5:])
                    assert source in (2, 5) and count > 0
                    error_reports.append((raw_mode, source, code, ms, count))
                    actual.pop()  # independent of the ordinary reply ordering assertions
                elif data[0] == 4 and len(data[5:]) != 1:
                    error_reads.append(struct.unpack("<BBIQ", data[5:]))
                else:
                    error_masks.append((data[0], data[5:]))
                    if data[0] == 3 and data[5:] == b"\x24":
                        # The next LSR read (including a TX poll) clears the flags.
                        uart_rx_fault = 0x12  # overrun and break
                        incoming.append(0)
                        incoming.extend(corrupt)
            elif method == 0x167 and data[5:] == bytes(2):
                assert actual[-2] == (6, bytes.fromhex("04670100000100")), (actual[-7:], list(wire_log))
                call(0x197, bytes.fromhex("010000000000"))
                call(0x19f, bytes.fromhex("0100"))
            elif method == 0x19f:
                assert actual[-2] == (6, bytes.fromhex("0497010000010082830100"))
                assert data[5:] == struct.pack("<HI", 1, 29104)
                extension(11)
            elif method == 0x544bf00b:
                if phase == "trace-burst-arm":
                    assert data[0] == 3 and data[5:] == b"\x1f"
                    phase = "trace-burst"
                    for p in range(len(PORTS)):
                        end = inject(BODY, p)
                        # Start the next peer after all 13 trailer/intermission
                        # bits, before a waiting gateway can claim an idle line.
                        next_sof = end - (18 - 14) * 2500
                        end = inject(bytes.fromhex("12300001 ca"), p, after=next_sof - 30_000)
                    ordered = sorted(edges)
                    edges.clear()
                    edges.extend(ordered)
                    trace_check_at, trace_resume = end + 10_000, end + 40_000
                elif phase == "trace-burst":
                    assert data[0] == 3 and data[5:] == b"\0"
                    assert len(burst_traces) == TRACE_BURST
                    for p in range(len(PORTS)):
                        assert [b for port, b in burst_traces if port == p] == [BODY, bytes.fromhex("12300001 ca")]
                    phase = "normal-complete"
                    cpu.emu_stop()
                elif normal_reads:
                    assert data[0] == 3 and (phase, data[5:]) in (("normal-block", b"\x08"), ("edit-guard", b"\x00"))
                elif data[0] == 4 and phase == "trace-congested":
                    assert data[5:] == b"\x01" + struct.pack("<5Q", 2, 0, 0, 0, 0)
                    assert (6, bytes.fromhex("0407010000 0c00")) in actual
                    assert [r[-2] for k, r in traces if k == 0] == list(range(1, TRACE_BURST + 1))
                    assert [r for k, r in traces if k == 5] == [bytes.fromhex("01 0200000000000000")]
                    phase = "trace-reporting"
                    send(0)
                elif data[0] == 4:
                    assert data[5:] == bytes(41)
                    extension(11, b"\x01", True)
                elif data[5:] == b"\x01":
                    phase = "trace-invalid"
                    inject(BODY, 0, bad_crc=True)
                else:
                    assert data[5:] == b"\0" and phase == "device-target"
                    assert (0, bytes.fromhex("01 76f00608 20032a0a06000000")) in traces
                    assert (0, bytes.fromhex("01 7d700602 8fe1")) in traces
                    assert (1, bytes.fromhex("00 00 6d700007 0300 00 00008000")) in traces
                    assert (1, bytes.fromhex("00 02 6df00007 0018 0300 020309")) in traces
                    assert (1, bytes.fromhex("01 00") + clock_frame) in traces
                    assert not any(k == 0 and record[1:] == clock_frame for k, record in traces)
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
                    setting_exchange.append((bytes.fromhex("6d700007 2600 00 00008000"),
                                             bytes.fromhex("6e700604 260001 05")))
                    inject(bytes.fromhex("62f0060b 0610 3406 1006 0000000000"))
                elif phase == "group":
                    for exchange in exchanges:
                        exchange.append((bytes.fromhex("61f00002 0c01"), None))
                    call(0x13d,b"\x0c\x01",True)
                    call(0x13d,b"\x0c\0")
                else:
                    phase = "fault-burst"
                    # Two lines finish at the same instant: the second report
                    # must not overwrite the first before UART can drain it.
                    faults = (bytes.fromhex("7d700602 8fe1"), bytes.fromhex("7d705002 0080"))
                    widths = [len(protected_bits(body)) for body in faults]
                    for port, body in enumerate(faults):
                        # Let the preceding CRC error flag and its delimiter finish.
                        inject(body, port, after=clock + 100_000 + (max(widths) - widths[port]) * 2500)
                    ordered = sorted(edges)
                    edges.clear()
                    edges.extend(ordered)
            elif method == 0x107 and data[0] == 2 and data[6:] == b"\0" and 1 <= data[5] <= TRACE_BURST + 2:
                congestion_reports.append(data[5])
            elif method == 0x107 and phase == "fault-burst":
                if data[0] == 2:
                    fault_words.append(data[5:])
                    assert fault_words == [b"\x8f\xe1", b"\x00\x80"][:len(fault_words)]
                    if len(fault_words) == 2:
                        call(0x107, bytes(2))
                else:
                    assert data == bytes.fromhex("0407010000 0080")
                    phase = "capture"
                    inject(bytes.fromhex("76f00608 20032a0a06000000"))
            elif method == 0x12f and data[0] == 2 and phase == "capture":
                assert data[5:] == bytes.fromhex("010003")
                assert (6,bytes.fromhex("023701000001000a06")) in actual
                assert (6, bytes.fromhex("021f01000001000300")) in actual
                phase = "live-cool-target"
            elif method == 0x147 and phase == "live-cool-target":
                # No block 5 or 6 read: only the member supplied this target.
                assert data == bytes.fromhex("0247010000 0100 00 2a")
                assert (6, bytes.fromhex("023f010000 0100 00 ff")) in actual
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
            elif method == 0x127 and phase == "setpoint-mode":
                assert data[5:] == bytes.fromhex("0100ff")
                if data[0] == 4:
                    call(0x127, bytes.fromhex("010002"), True)
                else:
                    assert data[0] == 3
                    phase = "await-setpoint-setup"
            elif method == 0x127 and phase == "clock":
                assert not setting_exchange and data[5:] == bytes.fromhex("010001")
                mode_services.append(data[0])
                if len(mode_services) == 3:
                    assert sorted(mode_services) == [3, 4, 4]
                    phase = "hot-mode-read"
                    call(0x127, bytes.fromhex("0100"))
            elif method == 0x127 and phase == "hot-mode-read":
                # No simulated peer reply: this must come from the confirmed mode.
                assert data == bytes.fromhex("0427010000 010001")
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
                extension(11, b"\0", True)
            elif method == 0x13e and data[0] == 2:
                if phase == "device-target":
                    assert data[5:] == bytes.fromhex("0100033406")
                    phase = "setpoint-mode"
                    call(0x127, bytes.fromhex("0100"))
                    return
                assert phase == "await-setpoint-setup" and data[5:] == bytes.fromhex("0100033506")
                assert not setting_exchange
                phase = "device-target-write"
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
            elif method == 0x17f and phase == "setback-before-status":
                assert encode(kind, data) == bytes.fromhex("ca0806047f0100000100009335")
                phase = "setback-member"
                inject(bytes.fromhex("6f700603 c00100"))
            elif method == 0x17f and phase == "setback-member":
                assert encode(kind, data) == bytes.fromhex("ca0806027f0100000100039435")
                phase = "setback-echo"
                call(0x17f, bytes.fromhex("0100ff"), True)
            elif method == 0x17f and phase == "setback-echo":
                assert encode(kind, data) == bytes.fromhex("ca0806037f0100000100039535")
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
                extension(12)
                incoming.extend(corrupt)
                call(0x187, bytes(2))
                panel_edges.extend([(clock + 200_000, True), (clock + 300_000, False)])
                extension(9, bytes([1 << 2]), True)
            elif method == 0x544BF009:
                assert raw_mode and data[0] == 3 and data[5:] == bytes([1 << 2])
                raw_ack_mask = data[5]
                plant_inputs()
            elif method == 0x544BF002 and data[5:] == bytes([0]):
                raw_mode, raw_ack_mask = False, 0
                extension(12, bytes.fromhex("0584"))
                extension(12, b"\0", True)
                extension(7)
            elif method == 0x544BF007:
                _, inputs, presses, _, _, _, sweep = struct.unpack("<2I2Q3I", data[5:])
                assert presses == 4 and inputs == 1 and not sweep
                assert (6, bytes.fromhex("0406f04b54 87010000 03")) in actual
                # Request ISP *during* the resulting flag at either UART rate.
                phase = "await-isp-flag"
                external[2] = True
                level(2, True, clock)
                expected_flags.add((2, clock + 6 * 2500))
            elif method == 0x544BF006 and data[5:] == bytes.fromhex("05f04b54 01"):
                assert phase in ("isp-during-flag", "enter-update") and retry_isp_at is None
                if phase == "isp-during-flag":
                    phase = "isp-after-flag"
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
            if phase == "trace-reporting":
                assert data == bytes.fromhex("02000103")  # still Normal: injection refused
                call(0x10f, b"\x01", True)
                return
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
    incoming.clear()
    training = deque()
    training_start = 0

    def connect_host():
        nonlocal training_start
        training_start = clock
        if fast:
            training.extend(clock + t for t in (10_000, 18_000, 90_000, 98_000, 170_000, 250_000))
        else:
            incoming.extend(queued)

    if not normal_reads:
        connect_host()
    acr = 0
    dlab = False
    dll = dlm = draining = 0
    entered = False

    def read(uc, off, size, user):
        nonlocal draining, acr, dll, dlm, uart_rx_fault
        if training and clock >= training[0]:
            at = training.popleft() - training_start
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
            flags, uart_rx_fault = uart_rx_fault, 0
            return (
                (0 if draining > 1 or clock < trace_resume else 0x20)
                | (0 if draining else 0x40)
                | bool(incoming)
                | flags
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
    # This is a runaway-code guard (including a loop that never reads a timer),
    # not a bus timing limit. The timer above and the individual wire assertions
    # bound simulated time, arbitration and ACK-slot timing independently.
    cpu.emu_start(base, 0xFFFFFFFF, count=750_000_000)
    painted = bytes(cpu.mem_read(floor, top - floor))
    free = len(painted) - len(painted.lstrip(b"\xa5"))
    assert free >= 256, free
    if normal_reads:
        assert boot_port_checked
        assert phase == "normal-complete" and not traces and not any(exchanges), (phase, actual[-8:])
        assert received_rx_acks == expected_rx_acks and observed_flags == expected_flags
        print(f"ARM Normal: offline counters, addressed reads, multipart status, guarded writes, per-port announcements, silent peer; untouched stack {free} bytes")
        return
    assert entered, (clock, phase, [len(x) for x in exchanges], results, actual[-10:],
                     line_sends, expected_rx_acks - received_rx_acks, list(wire_log))
    assert actual[:4] == [
        (6, bytes.fromhex(x))
        for x in ("048f0100000300", "0400000000", "0401f04b54020005bf3e")
    ] + [
        (
            6,
            bytes.fromhex("0404f04b54")
            + os.getenv("TEKMAR_BUILD_REVISION", "unversioned").encode(),
        )
    ]
    assert (6, bytes.fromhex("0306f04b5402f04b5401")) in actual
    assert error_masks == [(4, b"\0"), (3, b"\x24"), (4, b"\x24"), (3, b"\0")], error_masks
    assert [(s, k, n) for s, k, _, n in error_reads] == [(5, 0x84, 1), (5, 0x84, 3)], error_reads
    assert [(raw, n) for raw, s, k, _, n in error_reports if (s,k) == (5,0x84)] == [(False,2), (True,3)]
    assert [(k,n) for _,s,k,_,n in error_reports if s == 5 and k < 0x84] == [(0x80,1), (0x83,1)]
    assert {raw for raw, s, k, _, n in error_reports if (s,k) == (2,2)} == {False,True}
    assert phase == "enter-update" and (6, bytes.fromhex("0306f04b5405f04b5401")) in actual
    for p in range(5):
        assert (6, bytes.fromhex("0403f04b54") + bytes([p]) + bytes(16)) in actual
    capture, = (data[5:] for kind, data in actual if kind == 6 and data[:5] == bytes.fromhex("040af04b54"))
    assert all(struct.unpack("<2I5Q5I", capture)[2:7])
    assert stage_counts == [(1, 0, 1, 0)], stage_counts
    assert not reads and not read_events and actual[-1] == (6, bytes.fromhex("0305f04b54"))
    assert expected_flags == observed_flags, (clock, expected_flags - observed_flags, observed_flags - expected_flags)
    assert tuple(telemetry) == TELEMETRY
    assert congestion_reports == list(range(1, TRACE_BURST + 3)), congestion_reports
    assert [b for b, _ in line_sends[2]] == [init, master]
    assert not conflicts and not interrupted and lost == {"master", "write", "raw"}
    assert (1, raw_winner) in [(b[1], b[6:]) for k, b in actual if k == 1 and b[0] == 0]
    assert expected_rx_acks and received_rx_acks == expected_rx_acks, expected_rx_acks - received_rx_acks
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
    run(args.image, args.ram, args.fast, normal_reads=True)
