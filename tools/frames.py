"""Parse the passive receiver's UART records and independently verify the bus CRC."""

import argparse
import re
from collections.abc import Iterable, Iterator
from collections import Counter
from pathlib import Path

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


def dropped_records(line: bytes) -> int | None:
    match = re.fullmatch(rb"END ([0-9a-f]{8})\r\n", line)
    return int(match[1], 16) if match else None


class FrameAudit:
    """Compare received frame records against each port's independent decoder totals."""

    def __init__(self, require_stats=False):
        self.require_stats = require_stats
        self.seen = [Counter() for _ in range(5)]
        self.drops = {}
        self.tx_result = None

    def record(self, line: bytes) -> str:
        if line.startswith(b"T "):
            match = re.fullmatch(rb"T ([0-4]) (ACK|NOACK|ARBITRATION|ECHO|BAD-END|BUSY|LATE|PIN-BUSY|STOPPED)\r\n", line)
            if not match or self.drops or self.tx_result is not None:
                raise ValueError(f"invalid or duplicate TX result: {line!r}")
            self.tx_result = int(match[1]), match[2].decode("ascii")
            return f"port={self.tx_result[0]} tx_result={self.tx_result[1]}"
        if not line.startswith(b"D "):
            if self.drops:
                raise ValueError("frame received after diagnostic summaries")
            result = describe(line)
            _, port, _, _, status, _ = line.split()
            self.seen[int(port)][status] += 1
            return result
        if not re.fullmatch(rb"D [0-4](?: [0-9a-f]{8}){12,13}\r\n", line):
            raise ValueError("malformed receiver diagnostic")
        _, p, *words = line.split()
        port = int(p)
        edges, starts, stuff, crc, ack, no_ack, bad, partial, drops, lag, rx, expected, *levels = (
            int(word, 16) for word in words)
        if levels and levels[0] > 3:
            raise ValueError(f"port {port}: invalid input level flags")
        seen = self.seen[port]
        observed_bad = sum(seen[k] for k in (b"D", b"E", b"M"))
        if port in self.drops or partial > 1 or starts != stuff + crc + ack + no_ack + bad + partial:
            raise ValueError(f"port {port}: inconsistent candidate accounting")
        if (sum(seen.values()) + drops != ack + no_ack + bad or
                seen[b"A"] > ack or seen[b"N"] > no_ack or observed_bad > bad):
            raise ValueError(f"port {port}: frame records disagree with receiver totals")
        self.drops[port] = drops
        result = (f"port={port} edges={edges} candidates={starts} ack={ack} no_ack={no_ack} "
                  f"bad_trailer={bad} stuffing_errors={stuff} crc_errors={crc} partial={partial} "
                  f"uart_drops={drops} max_capture_service_us={lag}")
        if levels:
            start, end = levels[0] & 1, levels[0] >> 1
            result += f" rx_initial={start} rx_final={end}"
            if not edges and start == end:
                result += " line=steady-dominant" if end else " line=steady-idle"
        return result + (f" last_crc={rx:04x} expected={expected:04x}" if crc else "")

    def finish(self, dropped):
        if self.require_stats or self.drops:
            if len(self.drops) != 5 or sum(self.drops.values()) != dropped:
                raise ValueError("missing port summary or inconsistent UART drop count")
        if dropped:
            raise ValueError(f"gateway UART queue dropped {dropped} records")


def replay(lines: Iterable[bytes]) -> Iterator[str]:
    dropped = None
    audit = FrameAudit()
    for number, line in enumerate(lines, 1):
        if dropped is not None:
            raise ValueError(f"line {number}: unexpected data after END")
        dropped = dropped_records(line)
        if dropped is None:
            try:
                yield audit.record(line)
            except ValueError as exc:
                raise ValueError(f"line {number}: {exc}") from exc
    if dropped is None:
        raise ValueError("incomplete capture: missing END")
    audit.finish(dropped)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Decode a saved UART frame stream without a gateway.")
    parser.add_argument("capture", type=Path)
    args = parser.parse_args()
    try:
        with args.capture.open("rb") as src:
            for record in replay(src):
                print(record)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{args.capture}: {error}\n")
