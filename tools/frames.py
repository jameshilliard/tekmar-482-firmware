"""TPCK binary capture decoding and replay for the board utilities."""

import argparse
from pathlib import Path

from payloads import details

SOF, EOF, ESC = 0xCA, 0x35, 0x2F


def encode(kind, payload):
    interior = bytes((len(payload), kind)) + payload
    out = bytearray([SOF])
    for b in interior + bytes([sum(interior) & 255]):
        if b in (SOF, EOF, ESC):
            out.append(ESC)
        out.append(b)
    out.append(EOF)
    return bytes(out)


class Decoder:
    def __init__(self):
        self.active = self.escaped = False
        self.buf = bytearray()

    def feed(self, chunk):
        for b in chunk:
            if not self.escaped and b == SOF:
                self.active = True
                self.buf.clear()
            elif not self.active:
                continue
            elif not self.escaped and b == ESC:
                self.escaped = True
            elif not self.escaped and b == EOF:
                self.active = False
                if (
                    len(self.buf) < 3
                    or self.buf[0] + 3 != len(self.buf)
                    or sum(self.buf[:-1]) & 255 != self.buf[-1]
                ):
                    raise ValueError("invalid TPCK length or checksum")
                yield self.buf[1], bytes(self.buf[2:-1])
            else:
                self.escaped = False
                self.buf.append(b)
                if len(self.buf) > 258:
                    raise ValueError("oversized TPCK packet")


def describe(data):
    if len(data) < 2 or data[1] >= 5:
        raise ValueError("invalid raw port or header")
    kind, port = data[:2]
    if kind == 0:
        body = data[6:]
        if len(body) < 4 or len(body) != 4 + (body[3] & 63):
            raise ValueError("invalid raw frame length")
        header = int.from_bytes(body[:4], "big")
        mid = (header >> 20) & 1023
        return (
            f"port={port} end_ms={int.from_bytes(data[2:6], 'little')} "
            f"id=0x{mid:03x} subnet={(header >> 12) & 255} node={(header >> 6) & 63} "
            f"body={body.hex()} {details(mid, body[4:])}"
        ).rstrip()
    if kind == 4 and len(data) >= 9:
        count = int.from_bytes(data[7:9], "little")
        if len(data) != 9 + (count + 7) // 8 or (
            count % 8 and data[-1] & ((1 << (8 - count % 8)) - 1)
        ):
            raise ValueError("invalid captured bit count or padding")
        return (
            f"port={port} end_ms={int.from_bytes(data[2:6], 'little')} invalid={data[6]} "
            f"bit_count={count} bits={data[9:].hex()}"
        )
    if kind == 2 and (data[2:] == b"\x00" or (len(data) == 4 and data[2] in (1, 2))):
        return f"port={port} tx_result={data[2:].hex()}"
    if kind == 3 and len(data) == 3:
        return f"port={port} port_error={data[2]}"
    raise ValueError("unknown or malformed raw service")


def replay(chunks):
    parser = Decoder()
    for chunk in chunks:
        for kind, payload in parser.feed(chunk):
            if kind == 1:
                yield describe(payload)
    if parser.active:
        raise ValueError("incomplete TPCK capture")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("capture", type=Path)
    with parser.parse_args().capture.open("rb") as stream:
        for row in replay(iter(lambda: stream.read(4096), b"")):
            print(row)
