# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Run a verified SRAM trial from a synchronized ISP session or the installed dev image."""

import argparse
import asyncio
import binascii
from pathlib import Path

from backup import check_part, expect, line, read_page, send, synchronize
from board import monitor, panel, probe, request
from check_image import check
from port import connect

BASE = 0x40001000


async def start_image(port, image):
    check(image, ram=True)
    image += bytes(-len(image) % 4)
    await check_part(port)
    await send(port, b"K")
    await expect(port, b"0")
    minor, major = int(await line(port)), int(await line(port))
    print(f"ROM version {major}.{minor}; uploading {len(image)} bytes to SRAM", flush=True)
    for offset in range(0, len(image), 4096):
        data = image[offset:offset + 4096]
        addr = BASE + offset
        await send(port, f"W {addr} {len(data)}".encode("ascii"))
        await expect(port, b"0")
        for start in range(0, len(data), 900):
            block = data[start:start + 900]
            for i in range(0, len(block), 45):
                encoded = binascii.b2a_uu(block[i:i + 45], backtick=True)
                await send(port, encoded.rstrip(b"\n"))
            await send(port, str(sum(block)).encode("ascii"))
            await expect(port, b"OK")
        if await read_page(port, addr, len(data)) != data:
            raise RuntimeError("SRAM readback differs; refusing to execute")
    print("SRAM readback verified; starting the image", flush=True)
    await send(port, b"U 23130")  # Required by the ROM for Go.
    await expect(port, b"0")
    await send(port, f"G {BASE} A".encode("ascii"))
    await expect(port, b"0")


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port")
    parser.add_argument("ram_bin", type=Path)
    parser.add_argument("--probe", action="store_true", help="observe receiver inputs before returning to ISP")
    parser.add_argument("--panel-test", action="store_true", help="cycle and read back each panel LED")
    parser.add_argument("--frames", type=int, metavar="SECONDS", help="monitor live frames from SRAM")
    parser.add_argument("--diagnostics", action="store_true", help="verify receiver totals with --frames")
    parser.add_argument("--tx", nargs=2, metavar=("BUS", "HEX"), help="send one native frame during --frames")
    parser.add_argument("--from-app", action="store_true",
                        help="enter ISP from the installed dev image and return to it after the trial")
    args = parser.parse_args()
    if args.frames is not None and args.frames < 1:
        parser.error("--frames must be positive")
    if (args.diagnostics or args.tx) and not args.frames:
        parser.error("--diagnostics and --tx require --frames")
    image = args.ram_bin.read_bytes()
    check(image, ram=True)
    async with connect(args.port) as port:
        if args.from_app:
            await request(port, True)  # checks the dev identity before entering ROM
            await asyncio.sleep(0.1)
            await synchronize(port)
        await start_image(port, image)
        await asyncio.sleep(0.1)  # The application resets the UART FIFO during startup.
        try:
            if args.panel_test:
                await panel(port, test=True)
            if args.probe:
                await probe(port)
            if args.frames:
                tx = (int(args.tx[0]), bytes.fromhex(args.tx[1])) if args.tx else None
                await monitor(port, args.frames, diagnostics=args.diagnostics, tx=tx)
        finally:
            await request(port, True)
            await asyncio.sleep(0.1)  # Let ROM enable autobaud before sending its sync byte.
            await synchronize(port)
            if args.from_app:
                await send(port, b"U 23130")
                await expect(port, b"0")
                await send(port, b"G 0 A")
                # The installed app can reset the UART before ROM finishes its reply.
                await asyncio.sleep(0.15)
    if args.from_app:
        async with connect(args.port) as port:
            await request(port, False)
    print("Trial complete. Flash was not written.")


if __name__ == "__main__":
    asyncio.run(main())
