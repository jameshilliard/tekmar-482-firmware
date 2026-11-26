# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Talk only to the development firmware. Never erase or program flash."""

import argparse
import asyncio
import sys
from contextlib import nullcontext
from pathlib import Path
from typing import NamedTuple

from bridge_diagnostics import LAYOUTS
from frames import describe, encode
from port import connect


class PanelState(NamedTuple):
    boot: int
    inputs: int
    presses: int
    changes: int
    leds: int
    pins: int
    test: int


async def call(port, method, body=b"", *, update=False, receive=None):
    key = (0x544bf000 + method).to_bytes(4, "little")
    request = encode(6, bytes([0 if update else 1]) + key + body)
    async def wait():
        await port.write(request)
        while True:
            kind, data = await port.packet()
            if kind == 1 and receive is not None:
                receive(data)
            if kind != 6 or data[:1] != bytes([3 if update else 4]):
                continue
            if data[1:5] == key:
                return data[5:]
            if data[1:5] == bytes.fromhex("06f04b54") and data[5:9] == key:
                if update and method in (2, 5) and data[9:] == b"\x01":
                    # The transition was rejected before it took effect.
                    await asyncio.sleep(0.025)
                    await port.write(request)
                    continue
                raise RuntimeError(f"management request rejected: {data.hex()}")
            if data[1:5] == bytes(4):
                raise RuntimeError(f"management request rejected: {data.hex()}")
    return await asyncio.wait_for(wait(), 5)


async def panel_state(port, layout):
    state = PanelState(*layout.panel.unpack(await call(port, 7)))
    if max(state.boot, state.inputs) > 3 or max(state.pins, state.leds) > 63 or state.test > 1:
        raise ValueError(f"invalid panel flags: {state}")
    if state.leds != state.pins:
        raise RuntimeError(f"panel GPIO readback mismatch: {state}")
    return state


async def check_panel_input(port, initial, which, seconds, layout):
    print("Use only controls accessible with the enclosure closed.", flush=True)
    if which == "test":
        if initial.inputs & 2 or initial.pins & 32 or initial.test or initial.presses == layout.maximum:
            raise RuntimeError("start with Test released, Test LED off and press counter available")
        print("Waiting for a Test press: hold for one second, then release. "
              "Watch the Test LED.", flush=True)
    elif which == "dip":
        if initial.changes >= layout.maximum - 1:
            raise RuntimeError("DIP counter cannot record two more changes")
        print(f"Waiting for Reporting DIP to change from {initial.inputs & 1} "
              "and return to its starting position.", flush=True)
    else:
        raise ValueError(f"unknown panel input: {which}")
    loop = asyncio.get_running_loop()
    deadline = loop.time() + seconds
    asserted = False
    lit = False
    while loop.time() < deadline:
        state = await panel_state(port, layout)
        if state.boot != initial.boot:
            raise RuntimeError("boot input state changed during panel test")
        if which == "test":
            presses = state.presses - initial.presses
            if presses not in (0, 1):
                raise RuntimeError(f"expected one Test press; counter changed by {presses}")
            if state.test:
                raise RuntimeError("software lamp sweep active during Test input check")
            if presses:
                asserted |= bool(state.inputs & 2)
                lit |= bool(state.pins & 32)
                if not state.inputs & 2:
                    if asserted and lit:
                        print("Test input: press, release, one counted press and Test LED verified.")
                        return
                    raise RuntimeError(f"incomplete Test evidence: saw pressed={asserted}, Test LED={lit}")
        else:
            changes = state.changes - initial.changes
            flipped = bool((state.inputs ^ initial.inputs) & 1)
            if changes not in (0, 1, 2) or flipped != (changes == 1):
                raise RuntimeError(f"DIP input disagrees with transition count: {changes}")
            if flipped and not asserted:
                print("Observed the opposite DIP position.", flush=True)
                asserted = True
            if changes == 2:
                if not asserted:
                    raise RuntimeError("both DIP transitions counted but the opposite level was not observed")
                print("Reporting DIP: opposite level and return to the original level verified.")
                return
        await asyncio.sleep(0.06)
    raise RuntimeError(f"no complete physical {which} input test in {seconds:g} seconds")


async def panel(port, test=False, *, check_input=None, seconds=60):
    caps = await identify(port)
    layout = LAYOUTS[caps[0]]
    state = await panel_state(port, layout)
    print(f"panel: DIP at boot={state.boot & 1} now={state.inputs & 1} "
          f"test_button={state.inputs >> 1} presses={state.presses} DIP_changes={state.changes} "
          f"LED_pin_mask=0x{state.pins:02x}")
    if check_input is not None:
        await check_panel_input(port, state, check_input, seconds, layout)
        return
    if not test:
        return
    if await call(port, 8, update=True):
        raise RuntimeError("lamp test did not start")
    observed = []
    deadline = asyncio.get_running_loop().time() + 5
    while asyncio.get_running_loop().time() < deadline:
        state = await panel_state(port, layout)
        if not state.test:
            break
        if not observed or observed[-1] != state.pins:
            observed.append(state.pins)
        await asyncio.sleep(0.06)
    if state.test or observed != [1, 2, 4, 8, 16, 32]:
        raise RuntimeError(f"incomplete LED sweep: {observed}")
    print("LED GPIO readback passed: bus 0, 1, 2, 3, 4, Test.")


async def identify(port):
    if port.baudrate != 9600:
        burst = bytes([0xca]) * max(1, port.baudrate // 1250)
        for preamble in (burst, burst, bytes([0xca])):
            await port.write(preamble)
            await asyncio.sleep(0.08)
    caps = await call(port, 1)
    if len(caps) != 4 or caps[0] not in LAYOUTS or caps[1] > 1 or caps[2] != 5:
        raise RuntimeError("missing binary bridge capabilities")
    return caps


async def request(port, enter_isp):
    caps = await identify(port)
    if not enter_isp:
        print("Firmware build:", (await call(port, 4)).decode("ascii"))
        return
    if not caps[3] & 2 or await call(port, 5, update=True):
        raise RuntimeError("firmware update handoff not acknowledged")
    print("ROM ISP handoff acknowledged.")


async def probe(port):
    caps = await identify(port)
    capture = LAYOUTS[caps[0]].capture
    first = capture.unpack(await call(port, 10))
    await asyncio.sleep(1)
    second = capture.unpack(await call(port, 10))
    for p in range(5):
        print(f"Port {p}: {second[2+p]-first[2+p]} edges; last tick {second[7+p]}")


ECHO_QUERY = bytes.fromhex("6d70000705000000000000")


async def monitor(port, seconds, *, diagnostics=False, tx=None, on_record=None,
                  save_capture=None, ack_port=None, require_peer=True):
    caps = await identify(port)
    if not caps[3] & 1:
        raise RuntimeError("Raw mode not available")
    if tx and (tx[0] not in range(5) or len(tx[1]) < 4 or len(tx[1]) != 4 + (tx[1][3] & 63)):
        raise ValueError("invalid native frame")
    if ack_port is not None and ack_port not in range(5):
        raise ValueError("invalid ACK port")
    result = None
    with (save_capture.open("xb") if save_capture is not None else nullcontext()) as archive:
        if await call(port, 2, bytes([1]), update=True) != bytes([1]):
            raise RuntimeError("failed to enter Raw mode")
        def receive(data):
            nonlocal result
            row = describe(data)
            if archive:
                archive.write(encode(1, data))
            if on_record:
                on_record(data)
            if data[0] == 2:
                if not tx or tx[0] != data[1] or result is not None:
                    raise RuntimeError("unexpected terminal TX result")
                result = data[2:]
            print(row, flush=True)
        try:
            mask = bytes([0 if ack_port is None else 1 << ack_port])
            if await call(port, 9, mask, update=True, receive=receive) != mask:
                raise RuntimeError("ACK mask not acknowledged")
            if tx:
                await port.write(encode(1, bytes((1, tx[0])) + tx[1]))
            deadline = asyncio.get_running_loop().time() + seconds
            while (delay := deadline - asyncio.get_running_loop().time()) > 0 and (not tx or result is None):
                try:
                    kind, data = await asyncio.wait_for(port.packet(), delay)
                except asyncio.TimeoutError:
                    break
                if kind == 1:
                    receive(data)
        finally:
            if await call(port, 2, bytes([0]), update=True, receive=receive) != bytes([0]):
                raise RuntimeError("failed to leave Raw mode")
    if tx and result != bytes([0]) and (require_peer or result != bytes([2, 1])):
        raise RuntimeError(f"send not confirmed: {result!r}")
    if diagnostics:
        for p in range(5):
            actual, valid, invalid = LAYOUTS[caps[0]].drops.unpack(await call(port, 3, bytes([p])))
            if actual != p:
                raise RuntimeError("wrong stats port")
            print(f"port={p} dropped_valid={valid} dropped_invalid={invalid}")


async def echo_test(port, buses):
    for bus in buses:
        echoes = []
        await monitor(port, 8, tx=(bus, ECHO_QUERY), require_peer=False,
                      on_record=lambda data: echoes.append(data) if data[:2] == bytes((0,bus)) else None)
        if not any(data[6:] == ECHO_QUERY for data in echoes):
            raise RuntimeError(f"no matching bus echo on port {bus}")
        print(f"Port {bus}: transmit echo verified.")


async def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["identify", "probe", "echo-test", "frames", "diagnostics", "send", "panel", "led-test", "enter-isp"])
    parser.add_argument("port", help="local serial device (requires exclusive access)")
    parser.add_argument("--baud", type=int, default=9600)
    parser.add_argument("--seconds", type=int, default=30, help="capture or panel input test duration")
    parser.add_argument("--check-input", choices=("test", "dip"), help="verify a physical change with panel")
    parser.add_argument("--save-capture", type=Path, help="save binary Raw records in a new file")
    parser.add_argument("--ack-port", type=int, choices=range(5),
                        help="with diagnostics, acknowledge CRC-verified frames on just this bus")
    parser.add_argument("--tx", nargs=2, metavar=("BUS", "HEX"), help="one native frame, for send only")
    parser.add_argument("--bus", type=int, choices=range(5), help="test just this physical port with echo-test")
    args = parser.parse_args()
    if args.seconds < 1:
        parser.error("--seconds must be positive")
    if (args.action == "send") != bool(args.tx):
        parser.error("send requires --tx BUS HEX")
    if args.bus is not None and args.action != "echo-test":
        parser.error("--bus is only for echo-test")
    if args.save_capture and args.action not in ("send", "frames", "diagnostics"):
        parser.error("--save-capture requires send, frames or diagnostics")
    if args.ack_port is not None and args.action != "diagnostics":
        parser.error("--ack-port requires diagnostics")
    if args.check_input and args.action != "panel":
        parser.error("--check-input requires panel")
    try:
        async with connect(args.port, args.baud) as port:
            if args.action == "echo-test":
                await echo_test(port, range(5) if args.bus is None else [args.bus])
            elif args.action == "send":
                await monitor(port, args.seconds, tx=(int(args.tx[0]), bytes.fromhex(args.tx[1])),
                              save_capture=args.save_capture)
            elif args.action in ("frames", "diagnostics"):
                await monitor(port, args.seconds, diagnostics=args.action == "diagnostics",
                              save_capture=args.save_capture, ack_port=args.ack_port)
            elif args.action in ("panel", "led-test"):
                await panel(port, args.action == "led-test", check_input=args.check_input, seconds=args.seconds)
            elif args.action == "probe":
                await probe(port)
            else:
                await request(port, args.action == "enter-isp")
    except (OSError, RuntimeError, EOFError, ValueError, asyncio.TimeoutError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
