# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Run the emulator-checked SRAM image, then restart the identified custom firmware."""

import argparse
import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
from backup import expect, send, synchronize
from board import request
from check_image import check
from port import connect
from ram_trial import start_image


async def run(path, image):
    check(image, ram=True)
    async with connect(path) as port:
        await request(port, True)  # verifies the installed custom image before ISP
        await asyncio.sleep(0.1)
        await synchronize(port)
        await start_image(port, image)
        try:
            markers = await port.readline(16)
        finally:
            await asyncio.sleep(0.1)
            await synchronize(port)  # the trial itself returns to ROM
            await send(port, b"U 23130")
            await expect(port, b"0")
            await send(port, b"G 0 A")
            await asyncio.sleep(0.15)
    async with connect(path) as port:
        await request(port, False)
    if markers != b"FFSF\r\n":
        raise RuntimeError(f"unexpected task wake order: {markers!r}")
    print("Timed task wake order verified. Installed firmware restored; flash was not written.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port")
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    asyncio.run(run(args.port, args.image.read_bytes()))
