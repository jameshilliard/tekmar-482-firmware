# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Talk only to the development firmware. Never erase or program flash."""

import argparse
import asyncio
import binascii
import re
import sys
from contextlib import nullcontext
from pathlib import Path
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


def tx_command(bus, data):
    if not 0 <= bus <= 4 or len(data) < 4 or (data[3] & 63) + 4 != len(data):
        raise ValueError("expected port 0..4 and a complete tN4 header and payload")
    checksum = binascii.crc_hqx(bytes([bus]) + data, 0xffff)
    return f"A\nTX {bus} {data.hex()} {checksum:04x}\n".encode("ascii")


# Native BlockRequest for block 5; no target node bits set, no settings written.
ECHO_QUERY = bytes.fromhex("6d70000705000000000000")


class Transmission(NamedTuple):
    result: tuple[int, str] | None
    echoes: set[bytes]

    def verify(self, bus, *, require_peer=True):
        if self.result == (bus, "ACK"):
            expected = b"A"
        elif self.result == (bus, "NOACK") and not require_peer:
            expected = b"N"
        else:
            raise RuntimeError(f"transmit not acknowledged or incomplete: {self.result}")
        if expected not in self.echoes:
            raise RuntimeError("missing CRC-verified echo with matching trailer on the selected port")
        return expected == b"A"


async def capture(port, seconds: float, *, diagnostics=False, tx=None, on_record=None,
                  stop_on_result=False, archive=None, ack_port=None) -> Transmission:
    if ack_port is not None and (tx is not None or not 0 <= ack_port < 5):
        raise ValueError("receive ACK requires one port and cannot be combined with transmission")
    await identify(port)
    audit = FrameAudit(require_stats=diagnostics or tx is not None or ack_port is not None)
    command, greeting = ((b"A\nRX-DIAG\n", b"OK DIAG\r\n") if diagnostics else
                         (b"A\nRX-STREAM\n", b"OK FRAMES\r\n"))
    if tx is not None:
        command, greeting = tx_command(*tx), b"OK TX\r\n"
    elif ack_port is not None:
        command = f"A\nRX-ACK {ack_port}\n".encode()
        greeting = f"OK ACK {ack_port}\r\n".encode()
    await port.write(command)
    if await port.readline(64) != greeting:
        raise RuntimeError("frame stream did not start")
    echoes = set()
    def accept(line):
        print(audit.record(line), flush=True)
        if tx is not None and line.startswith(b"F "):
            _, bus, _, _, status, body = line.split()
            if int(bus) == tx[0] and bytes.fromhex(body.decode()) == tx[1]:
                echoes.add(status)
        if on_record is not None:
            on_record(line)
    loop = asyncio.get_running_loop()
    deadline = loop.time() + seconds
    async def read_record(deadline):
        line = await port.readline(160, timeout=max(0, deadline - loop.time()))
        if archive is not None:
            archive.write(line)
        return line
    try:
        while loop.time() < deadline:
            try:
                line = await read_record(deadline)
            except asyncio.TimeoutError:
                break
            accept(line)
            if stop_on_result and audit.tx_result is not None:
                break
    finally:
        await port.write(b"\n")
        deadline = loop.time() + 4
        while True:
            line = await read_record(deadline)
            if (dropped := dropped_records(line)) is not None:
                audit.finish(dropped)
                break
            accept(line)
    if ack_port is not None and (not audit.ack_summary or audit.ack_summary[0] != ack_port):
        raise ValueError("missing receive-ACK summary for selected port")
    if ack_port is None and audit.ack_summary is not None:
        raise ValueError("unexpected receive-ACK summary")
    return Transmission(audit.tx_result, echoes)


async def monitor(port, seconds: float, *, diagnostics=False, tx=None, on_record=None,
                  save_capture: Path | None = None, ack_port=None) -> None:
    with (save_capture.open("xb") if save_capture is not None else nullcontext()) as archive:
        evidence = await capture(port, seconds, diagnostics=diagnostics, tx=tx, on_record=on_record,
                                 archive=archive, ack_port=ack_port)
    if tx is not None:
        evidence.verify(tx[0])


async def echo_test(port, buses):
    failures = []
    for bus in buses:
        evidence = await capture(port, 8, tx=(bus, ECHO_QUERY), stop_on_result=True)
        try:
            peer = evidence.verify(bus, require_peer=False)
        except RuntimeError as error:
            failures.append(f"port {bus}: {error}")
        else:
            print(f"Port {bus}: transmit/receive echo verified; peer ACK {'seen' if peer else 'not seen'}.")
    if failures:
        raise RuntimeError("; ".join(failures))


async def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["identify", "probe", "echo-test", "frames", "diagnostics", "send", "panel", "led-test", "enter-isp"])
    parser.add_argument("port", help="local serial device (requires exclusive access)")
    parser.add_argument("--baud", type=int, default=9600)
    parser.add_argument("--seconds", type=int, default=30, help="frame capture duration")
    parser.add_argument("--save-capture", type=Path, help="save the raw UART stream in a new file")
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
