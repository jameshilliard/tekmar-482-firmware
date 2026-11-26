# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Exercise the real host helper against a pseudo-terminal, never a serial device."""

import os
import select
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HELPER = Path(__file__).resolve().parents[1] / "tools/board.py"
sys.path.insert(0, str(HELPER.parent))
from frames import describe
from board import Transmission


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
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        target = Path(directory.name) / "native.frames"
        master, proc = self.start_helper("frames", "--seconds", "1", "--save-capture", str(target))
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
        raw = target.read_bytes()
        self.assertEqual(raw, b"F 2 000061a8 0e2f A 12300001a5\r\n"
                              b"F 0 00010000 0e2f M 12300001a5\r\nEND 00000000\r\n")
        _, proc = self.start_helper("frames", "--save-capture", str(target))
        _, err = proc.communicate(timeout=3)
        self.assertNotEqual(proc.returncode, 0, err)
        self.assertEqual(target.read_bytes(), raw)

    def test_corrupt_frame_records(self):
        for raw in (b"12300001", b"12300001a4", b"z", b"ff" * 68):
            with self.assertRaises(ValueError):
                describe(b"F 2 000061a8 0e2f A " + raw + b"\r\n")

    def test_send_requires_acknowledged_echo(self):
        for echoed in (False, True):
            with self.subTest(echoed=echoed):
                master, proc = self.start_helper("send", "--seconds", "1", "--tx", "0", "12300001a5")
                self.expect(master, b"A\nID?\n")
                os.write(master, b"TEKMAR-482-DEV\r\n")
                self.expect(master, b"A\nTX 0 12300001a5 b45e\n")
                os.write(master, b"OK TX\r\n")
                if echoed:
                    os.write(master, b"F 0 00009c40 0e2f A 12300001a5\r\n")
                os.write(master, b"T 0 ACK\r\n")
                self.expect(master, b"\n")
                for port in range(5):
                    ack = int(echoed and port == 0)
                    counts = [30 * ack, ack, 0, 0, ack, 0, 0, 0, 0, 12, 0, 0]
                    os.write(master, (f"D {port}" + "".join(f" {n:08x}" for n in counts) + "\r\n").encode())
                os.write(master, b"END 00000000\r\n")
                _, err = proc.communicate(timeout=3)
                self.assertEqual(proc.returncode, 0 if echoed else 1, err)
                if not echoed:
                    self.assertIn(b"missing CRC-verified", err)

    def test_unpopulated_port_echo(self):
        master, proc = self.start_helper("echo-test", "--bus", "4")
        self.expect(master, b"A\nID?\n")
        os.write(master, b"TEKMAR-482-DEV\r\n")
        self.expect(master, b"A\nTX 4 6d70000705000000000000 2d3c\n")
        os.write(master, b"OK TX\r\nF 4 00010000 2d3f N 6d70000705000000000000\r\nT 4 NOACK\r\n")
        self.expect(master, b"\n")
        for port in range(5):
            n = int(port == 4)
            counts = (40*n, n, 0, 0, 0, n, 0, 0, 0, 16, 0, 0)
            os.write(master, (f"D {port}" + "".join(f" {v:08x}" for v in counts) + "\r\n").encode())
        os.write(master, b"END 00000000\r\n")
        out, err = proc.communicate(timeout=3)
        self.assertEqual(proc.returncode, 0, err)
        self.assertIn(b"Port 4: transmit/receive echo verified; peer ACK not seen", out)

    def test_noack_is_not_a_successful_send(self):
        with self.assertRaises(RuntimeError):
            Transmission((2, "NOACK"), {b"N"}).verify(2)
        for result, echoes in [((1, "ACK"), {b"A"}), ((2, "BAD-END"), {b"M"}),
                               ((2, "NOACK"), {b"A"}), (None, {b"N"})]:
            with self.subTest(result=result), self.assertRaises(RuntimeError):
                Transmission(result, echoes).verify(2, require_peer=False)

if __name__ == "__main__":
    unittest.main()
