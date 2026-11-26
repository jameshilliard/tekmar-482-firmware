"""Parse the passive receiver's UART records and independently verify the bus CRC."""

import re

from payloads import details


TRAILERS = {
    b"A": "ack", b"N": "no-ack", b"D": "bad-crc-delimiter",
    b"E": "ack/bad-end", b"M": "no-ack/bad-end",
}

def describe(line: bytes) -> str:
    match = re.fullmatch(
        rb"F ([0-4]) ([0-9a-f]{8}) ([0-7][0-9a-f]{3}) ([ANDEM]) ((?:[0-9a-f]{2}){4,67})\r\n",
        line,
    )
    if match is None:
        raise ValueError(f"invalid frame record: {line!r}")
    port, stamp, check, status, raw = match.groups()
    data = bytes.fromhex(raw.decode("ascii"))
    header = int.from_bytes(data[:4], "big")
    if header & 63 != len(data) - 4:
        raise ValueError("tN4 header length differs from UART record length")
    crc = 0x4599
    for byte in data:
        crc ^= byte << 7
        for _ in range(8):
            crc = ((crc << 1) ^ (0x4599 if crc & 0x4000 else 0)) & 0x7fff
    if crc != int(check, 16):
        raise ValueError("tN4 CRC differs from UART record data")
    message = (header >> 20) & 1023
    return (f"port={int(port)} t={int(stamp, 16)}us priority={header >> 30} "
            f"id=0x{message:03x} subnet=0x{(header >> 12) & 255:02x} "
            f"node=0x{(header >> 6) & 63:02x} payload={data[4:].hex() or '-'} "
            f"crc=0x{crc:04x} {TRAILERS[status]} {details(message, data[4:])}").rstrip()
