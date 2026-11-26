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


if __name__ == "__main__":
    unittest.main()
