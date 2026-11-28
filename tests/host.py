# /// script
# requires-python = ">=3.10"
# dependencies = ["serialx==1.8.1"]
# ///
"""Exercise the real host helper against a pseudo-terminal, never a serial device."""

import os
import select
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HELPER = Path(__file__).resolve().parents[1] / "tools/board.py"
sys.path.insert(0, str(HELPER.parent))
from frames import encode
from board import PanelState, ECHO_QUERY


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

    def exchange(self, master, method, reply, body=b"", *, update=False):
        mid = struct.pack("<I", 0x544bf000 + method)
        self.expect(master, encode(6, bytes([0 if update else 1]) + mid + body))
        os.write(master, encode(6, bytes([3 if update else 4]) + mid + reply))

    def capabilities(self, master):
        self.exchange(master, 1, bytes([3, 0, 5, 3]))

    def test_handoff(self):
        master, proc = self.start_helper("enter-isp")
        self.capabilities(master)
        key = struct.pack("<I", 0x544bf005)
        self.expect(master, encode(6, b"\0" + key))
        os.write(master, encode(6, bytes.fromhex("0306f04b54") + key + b"\x01"))
        self.exchange(master, 5, b"", update=True)
        _, err = proc.communicate(timeout=3)
        self.assertEqual(proc.returncode, 0, err)

    def test_non_busy_handoff_rejection_is_final(self):
        master, proc = self.start_helper("enter-isp")
        self.capabilities(master)
        key = struct.pack("<I", 0x544bf005)
        self.expect(master, encode(6, b"\0" + key))
        os.write(master, encode(6, bytes.fromhex("0306f04b54") + key + b"\x02"))
        _, err = proc.communicate(timeout=3)
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn(b"management request rejected", err)
        self.assertFalse(select.select([master], [], [], 0)[0])

    def test_identify(self):
        master, proc = self.start_helper("identify")
        self.capabilities(master)
        self.exchange(master, 4, b"unversioned")
        out, err = proc.communicate(timeout=3)
        self.assertEqual(proc.returncode, 0, err)
        self.assertIn(b"unversioned", out)

    def panel_check(self, which, states):
        master, proc = self.start_helper("panel", "--check-input", which, "--seconds", "5")
        self.capabilities(master)
        for state in states:
            self.exchange(master, 7, struct.pack("<2I2Q3I", *state))
        out, err = proc.communicate(timeout=2)
        return proc.returncode, out + err

    def test_observed_button_requires_press_release_and_indicator(self):
        base = PanelState(1, 1, (1 << 32) - 1, 2, 7, 7, 0)
        pressed = base._replace(inputs=3, presses=1 << 32, leds=39, pins=39)
        end = pressed._replace(inputs=1)
        status, output = self.panel_check("test", [base, pressed, end])
        self.assertEqual(status, 0, output)
        self.assertIn(b"one counted press and Test LED verified", output)
        for states in ([base, end], [base, pressed._replace(leds=7, pins=7),
                                     end._replace(leds=7, pins=7)]):
            status, output = self.panel_check("test", states)
            self.assertNotEqual(status, 0, output)
            self.assertIn(b"incomplete Test evidence", output)

    def test_observed_dip_must_change_and_return(self):
        base = PanelState(0, 0, 1, 5, 2, 2, 0)
        other = base._replace(inputs=1, changes=6)
        restored = base._replace(changes=7)
        status, output = self.panel_check("dip", [base, other, restored])
        self.assertEqual(status, 0, output)
        self.assertIn(b"return to the original level verified", output)
        for last, message in [(restored, b"opposite level was not observed"),
                              (other._replace(changes=5), b"disagrees with transition count"),
                              (other._replace(leds=64, pins=64), b"invalid panel flags")]:
            status, output = self.panel_check("dip", [base, last])
            self.assertNotEqual(status, 0, output)
            self.assertIn(message, output)

    def test_without_update_capability_no_handoff(self):
        master, proc = self.start_helper("enter-isp")
        self.exchange(master, 1, bytes([2, 0, 5, 1]))
        _, err = proc.communicate(timeout=3)
        self.assertNotEqual(proc.returncode, 0, err)
        self.assertFalse(select.select([master], [], [], 0)[0])

    def test_retired_custom_version_stops_before_panel_or_rom_handoff(self):
        for action in ("panel", "enter-isp"):
            with self.subTest(action=action):
                master, proc = self.start_helper(action)
                self.exchange(master, 1, bytes([1, 0, 5, 7]))
                out, err = proc.communicate(timeout=3)
                self.assertNotEqual(proc.returncode, 0)
                self.assertIn(b"unsupported custom bridge wire version 1", out + err)
                self.assertFalse(select.select([master], [], [], 0)[0])

    def test_probe(self):
        master, proc = self.start_helper("probe")
        self.capabilities(master)
        self.exchange(master, 10, bytes(68))
        self.exchange(master, 10, struct.pack("<2I5Q5I", *range(6), (1 << 32) + 6, *range(7, 12)))
        out, err = proc.communicate(timeout=3)
        self.assertEqual(proc.returncode, 0, err)
        self.assertIn(b"Port 4: 4294967302 edges; last tick 11", out)

    def raw(self, master, mask=0):
        self.capabilities(master)
        self.exchange(master, 2, b"\x01\x01", b"\x01\x01", update=True)
        self.exchange(master, 9, bytes([mask]), bytes([mask]), update=True)

    def close_raw(self, master):
        self.exchange(master, 2, bytes(2), bytes(2), update=True)

    def test_select_base_and_restore_normal(self):
        for action, initial, selection in [
            ("raw-base", 0, b"\x01\0"),
            ("normal", 1, bytes(2)),
        ]:
            with self.subTest(action=action):
                master, proc = self.start_helper(action)
                self.exchange(master, 1, bytes([3, initial, 5, 3]))
                self.exchange(master, 2, selection, selection, update=True)
                _, err = proc.communicate(timeout=3)
                self.assertEqual(proc.returncode, 0, err)

    def test_binary_capture_and_drop_counters(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "capture.tpck"
            master, proc = self.start_helper("diagnostics", "--ack-port", "3",
                                             "--seconds", "1", "--save-capture", str(target))
            self.raw(master, 1 << 3)
            packets = encode(1, bytes.fromhex("0003190000007d7123420582"), literal_header=True)
            packets += encode(1, bytes.fromhex("04021900000002010080"), literal_header=True)
            os.write(master, packets)
            self.close_raw(master)
            for p in range(5):
                self.exchange(master, 3, struct.pack("<BQQ", p, p, 0), bytes([p]))
            out, err = proc.communicate(timeout=3)
            self.assertEqual(proc.returncode, 0, err)
            self.assertEqual(target.read_bytes(), packets)
            self.assertIn(b"port=2 end_ms=25 invalid=2 bit_count=1 bits=80", out)
            self.assertIn(b"port=4 dropped_valid=4 dropped_invalid=0", out)

    def test_send_and_echo(self):
        for action, args, result, success in [
            ("echo-test", ["--bus", "4"], b"\x02\x01", True),
            ("send", ["--tx", "4", ECHO_QUERY.hex()], b"\x00", True),
            ("send", ["--tx", "4", ECHO_QUERY.hex()], b"\x02\x01", False),
        ]:
            master, proc = self.start_helper(action, *args)
            self.raw(master)
            self.expect(master, encode(1, bytes([0, 4]) + bytes(4) + ECHO_QUERY, literal_header=True))
            os.write(master, encode(1, bytes([0, 4]) + bytes(4) + ECHO_QUERY, literal_header=True)
                     + encode(1, bytes([2, 4]) + result, literal_header=True))
            self.close_raw(master)
            _, err = proc.communicate(timeout=3)
            self.assertEqual(proc.returncode == 0, success, err)


if __name__ == "__main__":
    unittest.main()
