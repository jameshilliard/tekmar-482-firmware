# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Read an unprotected LPC2146 in ISP mode. Never unlock, erase or write to the MCU."""

import argparse
import asyncio
import binascii
import hashlib
import struct
from pathlib import Path

from port import connect

FLASH_SIZE = 256 * 1024
PART_ID = 0x0402FF23


async def line(port):
    data = await port.readline()
    # Spaces in UU-encoded lines are payload, including trailing spaces.
    return data.rstrip(b"\r\n")


async def send(port, message):
    await port.write(message + b"\r\n")


async def expect(port, expected):
    actual = await line(port)
    if actual != expected:
        raise RuntimeError(f"expected {expected!r} from ISP, received {actual!r}")


async def synchronize(port):
    await port.write(b"?")
    await expect(port, b"Synchronized")
    for command, reply in [(b"Synchronized", b"OK"), (b"12000", b"OK"), (b"A 0", b"0")]:
        await send(port, command)
        await expect(port, command)
        await expect(port, reply)
    await check_part(port)


async def check_part(port):
    await send(port, b"J")
    await expect(port, b"0")
    actual = int(await line(port))
    if actual != PART_ID:
        raise RuntimeError(f"unexpected part ID: {actual:#010x}")


def decode_uu(data):
    if not data or not 0x21 <= data[0] <= 0x4d:
        raise ValueError("invalid UU byte count")
    count = data[0] - 0x20
    groups = (count + 2) // 3
    if len(data) != 1 + 4 * groups or any(not 0x20 <= b <= 0x60 for b in data[1:]):
        raise ValueError("invalid UU data")
    # The LPC2146 ROM fills the last quartet even when its last bytes are padding.
    return binascii.a2b_uu(bytes([0x20 + 3 * groups]) + data[1:])[:count]


async def read_page(port, addr, size):
    if not 0 < size <= 4096 or (size | addr) % 4:
        raise ValueError("expected an aligned ISP transfer of at most 4 KiB")
    await send(port, f"R {addr} {size}".encode("ascii"))
    await expect(port, b"0")
    result = bytearray()
    while len(result) < size:
        count = min(size - len(result), 900)
        block = bytearray()
        while len(block) < count:
            decoded = decode_uu(await line(port))
            if len(decoded) != min(45, count - len(block)):
                raise RuntimeError(f"unexpected data length at {addr + len(result):#x}")
            block.extend(decoded)
        if int(await line(port)) != sum(block):
            raise RuntimeError(f"ISP checksum mismatch at {addr + len(result):#x}; aborting")
        await send(port, b"OK")
        result.extend(block)
    return bytes(result)


async def read_flash(port):
    result = bytearray()
    for addr in range(0, FLASH_SIZE, 4096):
        result.extend(await read_page(port, addr, 4096))
        if len(result) % 65536 == 0:
            print(f"  {len(result) // 1024} KiB received", flush=True)
    return bytes(result)


def restore_image(first, second, vendor):
    if len(first) != FLASH_SIZE or first != second:
        raise ValueError("raw reads differ or are incomplete; no restore image")
    if not 0x200 <= len(vendor) <= FLASH_SIZE:
        raise ValueError("a flat stock application binary based at zero is required")
    if sum(struct.unpack_from("<8I", vendor)) & 0xffffffff:
        raise ValueError("vendor vector checksum is invalid")
    if first[64:len(vendor)] != vendor[64:]:
        raise ValueError("application differs from vendor image; preserving raw reads only")
    # Addresses 0..63 may be ROM vectors while ISP is active.
    return vendor[:64] + first[64:]


def save(directory, name, data):
    with (directory / name).open("xb") as output:
        output.write(data)
    digest = hashlib.sha256(data).hexdigest()
    print(f"{digest}  {name}", flush=True)
    return f"{digest}  {name}\n"


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port")
    parser.add_argument("vendor_bin", type=Path)
    parser.add_argument("new_directory", type=Path)
    parser.add_argument("--synced", action="store_true", help="reuse an idle, echo-off ISP session")
    args = parser.parse_args()
    vendor = args.vendor_bin.read_bytes()
    dest = args.new_directory
    dest.mkdir(mode=0o700, exist_ok=False)
    manifest = ""
    async with connect(args.port) as port:
        if args.synced:
            await check_part(port)
        else:
            await synchronize(port)
        print("LPC2146 confirmed. Starting read 1 of 2.", flush=True)
        first = await read_flash(port)
        manifest += save(dest, "isp-read-1.bin", first)
        print("Starting independent read 2 of 2.", flush=True)
        second = await read_flash(port)
        manifest += save(dest, "isp-read-2.bin", second)
    restore = restore_image(first, second, vendor)
    manifest += save(dest, "restore-with-stock-vectors.bin", restore)
    (dest / "SHA256SUMS").write_text(manifest, encoding="ascii")
    print("Verified both reads and stock application. No flash writes.")


if __name__ == "__main__":
    asyncio.run(main())
