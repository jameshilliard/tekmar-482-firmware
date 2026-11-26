# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Test the read-only ISP transcript and backup equivalence gates without hardware."""

import binascii
import sys
import unittest
from collections import deque
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import backup
import ram_trial


class Transcript:
    def __init__(self, lines):
        self.lines = deque(lines)
        self.sent = []

    async def write(self, data):
        self.sent.append(data)

    async def readline(self):
        return self.lines.popleft() + b"\r\n"


class BackupTest(unittest.IsolatedAsyncioTestCase):
    def test_rom_uu_final_quartet(self):
        self.assertEqual(backup.decode_uu(b"!Y>7E"), b"\xe5")
        for bad in (b"", b"!Y>", b"!Y>7E!", b"!Y>7\x7f"):
            with self.assertRaises(ValueError):
                backup.decode_uu(bad)

    async def test_one_page(self):
        data = bytes(range(256)) * 16
        lines = [b"0"]
        for start in range(0, len(data), 900):
            block = data[start:start + 900]
            for idx in range(0, len(block), 45):
                lines.append(binascii.b2a_uu(block[idx:idx + 45]).rstrip(b"\n"))
            lines.append(str(sum(block)).encode())
        port = Transcript(lines)
        with patch.object(backup, "FLASH_SIZE", len(data)):
            self.assertEqual(await backup.read_flash(port), data)
        self.assertEqual(port.sent, [b"R 0 4096\r\n"] + [b"OK\r\n"] * 5)
        lines[-1] = b"0"
        with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
            await backup.read_flash(Transcript(lines))

    async def test_read_protected(self):
        port = Transcript([b"19"])
        with self.assertRaises(RuntimeError):
            await backup.read_flash(port)
        self.assertEqual(port.sent, [b"R 0 4096\r\n"])

    def test_restore_preserves_tail_and_rejects_mismatch(self):
        vendor = bytes(64) + b"v" * 448
        raw = b"r" * 64 + vendor[64:] + b"x" * (backup.FLASH_SIZE - len(vendor))
        self.assertEqual(backup.restore_image(raw, raw, vendor), vendor[:64] + raw[64:])
        for other, ref in [(raw[:-1], vendor), (raw, vendor[:-1] + b"z")]:
            with self.assertRaises(ValueError):
                backup.restore_image(raw, other, ref)

    async def test_ram_trial_requires_correct_link_and_exact_readback(self):
        image_dir = Path(__file__).resolve().parents[1] / "target"
        image_path = Path("armv4t-none-eabi/release/tekmar-482-firmware.bin")
        flash = (image_dir / image_path).read_bytes()
        ram = (image_dir / "ram-trial" / image_path).read_bytes()
        untouched = Transcript([])
        with self.assertRaises(ValueError):
            await ram_trial.start_image(untouched, flash)
        self.assertEqual(untouched.sent, [])

        rx = [b"0", str(backup.PART_ID).encode(), b"0", b"13", b"2", b"0"]
        rx += [b"OK"] * ((len(ram) + 899) // 900)
        for readback in (ram[:-1] + bytes([ram[-1] ^ 1]), ram):
            port = Transcript(rx + [b"0", b"0"])
            with patch.object(ram_trial, "read_page", return_value=readback):
                if readback != ram:
                    with self.assertRaisesRegex(RuntimeError, "readback differs"):
                        await ram_trial.start_image(port, ram)
                    self.assertFalse(any(x.startswith((b"U ", b"G ")) for x in port.sent))
                else:
                    await ram_trial.start_image(port, ram)
                    self.assertEqual(port.sent[-2:], [b"U 23130\r\n", b"G 1073745920 A\r\n"])


if __name__ == "__main__":
    unittest.main()
