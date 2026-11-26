# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Talk only to the development firmware. Never erase or program flash."""

import argparse
import asyncio
import re
import sys
from typing import NamedTuple

from frames import FrameAudit, dropped_records
from port import connect


class PanelState(NamedTuple):
    boot: int
    inputs: int
    presses: int
    changes: int
    leds: int
    pins: int
    test: int


async def panel_state(port):
    await port.write(b"A\nPANEL?\n")
    line = await port.readline(100)
    if not re.fullmatch(rb"PANEL(?: [0-9a-f]{8}){7}\r\n", line):
        raise ValueError(f"invalid panel state: {line!r}")
    state = PanelState(*(int(v, 16) for v in line.split()[1:]))
    if state.leds != state.pins:
        raise RuntimeError(f"panel GPIO readback mismatch: {state}")
    return state


async def panel(port, test=False):
    await identify(port)
    state = await panel_state(port)
    print(f"panel: DIP at boot={state.boot & 1} now={state.inputs & 1} "
          f"test_button={state.inputs >> 1} presses={state.presses} DIP_changes={state.changes} "
          f"LED_pin_mask=0x{state.pins:02x}")
    if not test:
        return
    await port.write(b"A\nLED-TEST\n")
    if await port.readline() != b"OK LED\r\n":
        raise RuntimeError("lamp test did not start")
    observed = []
    deadline = asyncio.get_running_loop().time() + 5
    while asyncio.get_running_loop().time() < deadline:
        state = await panel_state(port)
        if not state.test:
            break
        if not observed or observed[-1] != state.pins:
            observed.append(state.pins)
        await asyncio.sleep(0.06)
    if state.test or observed != [1, 2, 4, 8, 16, 32]:
        raise RuntimeError(f"incomplete LED sweep: {observed}")
    print("LED GPIO readback passed: bus 0, 1, 2, 3, 4, Test.")


async def identify(port) -> str:
    await port.write(b"A\nID?\n")
    identity = await port.readline(64)
    if identity != b"TEKMAR-482-DEV\r\n":
        raise RuntimeError("no development firmware identity; no ISP command sent")
    return identity.decode("ascii").rstrip()


async def request(port, enter_isp: bool) -> None:
    identity = await identify(port)
    if not enter_isp:
        print(identity)
        return
    await port.write(b"A\nENTER-ISP\n")
    if await port.readline(64) != b"OK ISP\r\n":
        raise RuntimeError("no ISP acknowledgment; gateway state unknown, not retrying")
    print("ISP handoff acknowledged; the serial port is now available to the flasher.")


async def probe(port) -> None:
    await identify(port)
    await port.write(b"A\nRX-BEGIN\n")
    if await port.readline(64) != b"OK RX\r\n":
        raise RuntimeError("receiver capture did not start")
    await asyncio.sleep(1)
    await port.write(b"A\nRX-END\n")
    reply = await port.readline(96)
    if not re.fullmatch(rb"RX(?: [0-9a-f]{8}){8}\r\n", reply):
        raise ValueError(f"invalid capture reply: {reply!r}")
    t0, t1, seen, *stamps = (int(v, 16) for v in reply.split()[1:])
    if not t0 or not t1 or seen >= 1 << len(stamps):
        raise ValueError(f"invalid timer or port mask: {reply!r}")
    print(f"Timer 0: {t0} us; Timer 1: {t1} us")
    for idx, stamp in enumerate(stamps):
        active = bool(seen & (1 << idx))
        elapsed = t1 if idx in (1, 3) else t0
        if stamp > elapsed or (stamp and not active):
            raise ValueError(f"invalid capture timestamp for port {idx}: {stamp}")
        status = f"edge observed, last at {stamp} us" if active else "no edge observed"
        print(f"Port {idx}: {status}")


async def monitor(port, seconds: float, *, diagnostics=False) -> None:
    await identify(port)
    audit = FrameAudit(require_stats=diagnostics)
    await port.write(b"A\nRX-DIAG\n" if diagnostics else b"A\nRX-STREAM\n")
    if await port.readline(64) != (b"OK DIAG\r\n" if diagnostics else b"OK FRAMES\r\n"):
        raise RuntimeError("frame stream did not start")
    loop = asyncio.get_running_loop()
    deadline = loop.time() + seconds
    try:
        while (remaining := deadline - loop.time()) > 0:
            try:
                line = await port.readline(160, timeout=remaining)
            except asyncio.TimeoutError:
                break
            print(audit.record(line), flush=True)
    finally:
        await port.write(b"\n")
        deadline = loop.time() + 4
        while True:
            line = await port.readline(160, timeout=max(0, deadline - loop.time()))
            if (dropped := dropped_records(line)) is not None:
                audit.finish(dropped)
                break
            print(audit.record(line), flush=True)


async def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["identify", "probe", "frames", "diagnostics", "panel", "led-test", "enter-isp"])
    parser.add_argument("port", help="local serial device (requires exclusive access)")
    parser.add_argument("--baud", type=int, default=9600)
    parser.add_argument("--seconds", type=int, default=30, help="frame capture duration")
    args = parser.parse_args()
    if args.seconds < 1:
        parser.error("--seconds must be positive")
    try:
        async with connect(args.port, args.baud) as port:
            if args.action in ("frames", "diagnostics"):
                await monitor(port, args.seconds, diagnostics=args.action == "diagnostics")
            elif args.action in ("panel", "led-test"):
                await panel(port, args.action == "led-test")
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
