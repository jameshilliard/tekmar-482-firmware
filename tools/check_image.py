"""Check the LPC2146 boot contract on the exact bytes used to make the HEX."""

import argparse
import struct
from pathlib import Path


def check(image: bytes, ram: bool = False) -> None:
    base, capacity = (0x40001000, 0x4000) if ram else (0, 0x10000)
    if not 0x200 < len(image) <= capacity:
        raise ValueError("image outside the intended memory region")
    vectors = struct.unpack_from("<8I", image)
    if sum(vectors) & 0xFFFFFFFF:
        raise ValueError("invalid LPC2146 vector checksum")
    if image[0x1FC:0x200] != bytes.fromhex("ff ff ff ff"):
        raise ValueError("expected an erased code read protection word")
    if any(v != 0xE59FF018 for i, v in enumerate(vectors) if i != 5):
        raise ValueError("unexpected ARM7 vector instruction")
    for entry in struct.unpack_from("<8I", image, 0x20):
        if entry % 4 or not base + 0x200 <= entry < base + len(image):
            raise ValueError(f"vector points outside ARM code: {entry:#x}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--ram", action="store_true")
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    image = args.image.read_bytes()
    check(image, args.ram)
    print(f"{len(image)} bytes; vectors, protection word and image bounds OK")
