# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Run the RAM-linked development image in an already synchronized LPC2146 ISP session."""

import argparse
import asyncio
import binascii
from pathlib import Path

from backup import check_part, expect, line, read_page, send, synchronize
from board import request
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
    args = parser.parse_args()
    async with connect(args.port) as port:
        await start_image(port, args.ram_bin.read_bytes())
        await asyncio.sleep(0.1)  # The application resets the UART FIFO during startup.
        await request(port, True)
        await asyncio.sleep(0.1)  # Let ROM enable autobaud before sending its sync byte.
        await synchronize(port)
        print("ROM answered with the LPC2146 part ID after ENTER-ISP. Flash was not written.")


if __name__ == "__main__":
    asyncio.run(main())
