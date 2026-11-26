# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Exercise the real host helper against a pseudo-terminal, never a serial device."""

import os
import select
import subprocess
import sys
import unittest
from pathlib import Path

HELPER = Path(__file__).resolve().parents[1] / "tools/board.py"
sys.path.insert(0, str(HELPER.parent))
from frames import describe, details


class HelperTest(unittest.TestCase):
    def start_helper(self, action, *args):
        master, slave = os.openpty()
        for fd in (master, slave):
            self.addCleanup(os.close, fd)
        proc = subprocess.Popen(
            [sys.executable, str(HELPER), action, os.ttyname(slave), *args],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.addCleanup(proc.wait)
        self.addCleanup(proc.kill)
        return master, proc

    def expect(self, master, request):
        received = b""
        while len(received) < len(request):
            self.assertTrue(select.select([master], [], [], 3)[0], "request timed out")
            received += os.read(master, 128)
        self.assertEqual(received, request)

    def test_handoff(self):
        master, proc = self.start_helper("enter-isp", "--baud", "187500")
        self.expect(master, b"A\nID?\n")
        os.write(master, b"TEKMAR-482-DEV\r\n")
        self.expect(master, b"A\nENTER-ISP\n")
        os.write(master, b"OK ISP\r\n")
        _, err = proc.communicate(timeout=3)
        self.assertEqual(proc.returncode, 0, err)

    def test_identify(self):
        master, proc = self.start_helper("identify")
        self.expect(master, b"A\nID?\n")
        os.write(master, b"TEKMAR-482-DEV\r\n")
        out, err = proc.communicate(timeout=3)
        self.assertEqual(proc.returncode, 0, err)
        self.assertEqual(out.strip(), b"TEKMAR-482-DEV")

    def test_wrong_device(self):
        master, proc = self.start_helper("enter-isp")
        self.expect(master, b"A\nID?\n")
        os.write(master, b"unexpected identity\r\n")
        _, err = proc.communicate(timeout=3)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn(b"no ISP command sent", err)
        self.assertFalse(select.select([master], [], [], 0)[0])

    def test_probe(self):
        master, proc = self.start_helper("probe")
        self.expect(master, b"A\nID?\n")
        os.write(master, b"TEKMAR-482-DEV\r\n")
        self.expect(master, b"A\nRX-BEGIN\n")
        os.write(master, b"OK RX\r\n")
        self.expect(master, b"A\nRX-END\n")
        os.write(master, b"RX 000f4240 000f4241 00000003 00000000 0000000f"
                         b" 00000000 00000000 00000000\r\n")
        out, err = proc.communicate(timeout=3)
        self.assertEqual(proc.returncode, 0, err)
        self.assertIn(b"Timer 0: 1000000 us; Timer 1: 1000001 us", out)
        self.assertIn(b"Port 0: edge observed, last at 0 us", out)
        self.assertIn(b"Port 1: edge observed, last at 15 us", out)
        self.assertIn(b"Port 4: no edge observed", out)

    def test_frames(self):
        master, proc = self.start_helper("frames", "--seconds", "1")
        self.expect(master, b"A\nID?\n")
        os.write(master, b"TEKMAR-482-DEV\r\n")
        self.expect(master, b"A\nRX-STREAM\n")
        os.write(master, b"OK FRAMES\r\nF 2 000061a8 0e2f A 12300001a5\r\n")
        self.expect(master, b"\n")
        os.write(master, b"F 0 00010000 0e2f M 12300001a5\r\nEND 00000000\r\n")
        out, err = proc.communicate(timeout=3)
        self.assertEqual(proc.returncode, 0, err)
        self.assertIn(b"port=2 t=25000us priority=0 id=0x123", out)
        self.assertIn(b"payload=a5 crc=0x0e2f no-ack/bad-end", out)

    def test_corrupt_frame_records(self):
        for raw in (b"12300001", b"12300001a4", b"z", b"ff" * 68):
            with self.assertRaises(ValueError):
                describe(b"F 2 000061a8 0e2f A " + raw + b"\r\n")

    def test_payload_details(self):
        self.assertEqual(details(0x17e, bytes.fromhex("1101000000")),
                         "SnowmeltZoneStatus source_type=1 source_id=2 pairs=1")
        self.assertEqual(details(0x17e, bytes.fromhex("110300")), "SnowmeltZoneStatus")
        self.assertEqual(details(0x3d7, bytes.fromhex("0142")), "NetworkError code=1 instance=2 priority=1")
        self.assertEqual(details(0x337, b"\x64"), "MaximumBoilerPercent max_value=100")


if __name__ == "__main__":
    unittest.main()
