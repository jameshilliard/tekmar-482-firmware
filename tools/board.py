# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Talk only to the development firmware. Never erase or program flash."""

import argparse
import asyncio
import sys

from port import connect


async def request(port, enter_isp: bool) -> None:
    await port.write(b"A\nID?\n")
    identity = await port.readline(64)
    if identity != b"TEKMAR-482-DEV\r\n":
        raise RuntimeError("no development firmware identity; no ISP command sent")
    if not enter_isp:
        print(identity.decode("ascii").rstrip())
        return
    await port.write(b"A\nENTER-ISP\n")
    if await port.readline(64) != b"OK ISP\r\n":
        raise RuntimeError("no ISP acknowledgment; gateway state unknown, not retrying")
    print("ISP handoff acknowledged; the serial port is now available to the flasher.")


async def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["identify", "enter-isp"])
    parser.add_argument("port", help="local serial device (requires exclusive access)")
    parser.add_argument("--baud", type=int, default=9600)
    args = parser.parse_args()
    try:
        async with connect(args.port, args.baud) as port:
            await request(port, args.action == "enter-isp")
    except (OSError, RuntimeError, EOFError, ValueError, asyncio.TimeoutError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
