"""Native tN4 application layouts. Vectors are synthetic, not site captures."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from frames import FrameAudit, replay
from payloads import DECODERS, decode_payload


def decode(mid, hex_bytes):
    return decode_payload(mid, bytes.fromhex(hex_bytes))


class PayloadTests(unittest.TestCase):
    def test_known_layouts(self):
        cases = {
            0x17e: ("1102225006248104", dict(source_type=1, source_id=2, pairs=2,
                       pair0_key="0x22", pair0_slab_temperature_f=76.6,
                       pair1_key="0x24", pair1_edge_temperature_f=30.3)),
            0x1f2: ("3412020555aa", dict(subnet_id=52, node_id=18, channel_id=2,
                                         type_id=5, io_value=0xaa55)),
            0x22f: ("fbe7a406000080044e42de99", dict(enabled=1, setback_state=5,
                       priority=3, actuator_status=3, demand_status=1, heat_cool_mode=0,
                       operating_mode=2, stat_priority=3, target_temp_f=85.0,
                       temp1_raw=0, temp2_raw=0x480, aux_relay_status=2, reserved=1,
                       requested_enable_status=3, actual_enable_status=0,
                       status_excuse1=0x42, status_excuse2=0xde, extension_hex="99")),
            0x23f: ("a7f93151", dict(setback_state=3, optimum_start=2, priority=2,
                        actuator_status=1, heat_cool_mode=3, operating_mode=7,
                        target_temp_f=76.1, tank_temp_f=104.9)),
            0x24f: ("4306a4050a64aabbf2", dict(device_total=3, device_id=4,
                        plant="modulating_boiler", pump_status=2, state=4, reserved=2,
                        current_modulation_level=5, minimum_modulation_level=10,
                        maximum_modulation_level=100, input_temp_raw=170, outlet_temp_raw=187,
                        output=18, return_protection=7)),
            0x32f: ("32053205", dict(outdoor_temp_f=48.0, second_word_raw=1330, matches_first=1)),
            0x337: ("42", dict(max_value=66)),
            0x33f: ("0320", dict(air_group_id=3, requested_length=32)),
            0x367: ("2b7be33406", dict(water_subnet=43, water_bus=3, water_control=5,
                        stage=1, override_status=1, first_report=1, setpoint_control=1,
                        dhw_control=1, priority_level=1, pump_delay=35, pump_not_required=1,
                        output_on=1, target_temp_raw=1588)),
            0x36f: ("e3db2a0a060d00c0", dict(air_group_id=3, mode="cool",
                        request_for_master_report=1, reserved0=1, heat_cool_enabled=1,
                        heat_cool_demand=1, schedule_status=6, optimum_start=2,
                        weather_shutdown=1, output_1_status=1, output_2_status=3,
                        target_f=69.8, temperature_f=69.6, remote_temperature="unavailable")),
            0x3c7: ("2b", dict(water_bus=3, water_control=5)),
            0x3cf: ("340900c003493264fe", dict(supply_temp_raw=0x934,
                        return_temp="unavailable", target_temp_raw=0x903, heat_cool_mode=2,
                        loop_output_pct=50, pump_output_pct=100, setback_status=6,
                        weather_shutdown=1, operating_on_max=1, operating_on_min=1,
                        output_limited=1, reserved=1)),
            0x3d7: ("0582", dict(code=5, instance=2, priority=2)),
        }
        self.assertEqual(set(cases), DECODERS.keys())
        for mid, (body, expected) in cases.items():
            with self.subTest(mid=hex(mid)):
                fields = decode(mid, body)
                self.assertEqual({k: fields[k] for k in expected}, expected)

    def test_variants_and_extensions(self):
        self.assertEqual(decode(0x32f, "3405")["outdoor_temp_raw"], 0x534)
        self.assertEqual(decode(0x32f, "34053305")["matches_first"], 0)
        self.assertNotIn("target_temp_raw", decode(0x367, "090240"))
        self.assertEqual(decode(0x3cf, "340900c003493264fe3412")["dhw_supply_temp_raw"], 0x1234)
        self.assertEqual(decode(0x24f, "01eeaabb"),
                         dict(device_total=1, device_id=0, device_type=238, extension_hex="aabb"))
        fields = decode(0x17e, "11032f01002f0200ee1234")
        self.assertEqual((fields["pair0_keep_alive"], fields["pair1_keep_alive"]), (1, 2))
        self.assertEqual(fields["pair2_value_raw"], 0x3412)
        self.assertEqual(decode(0x337, "641234")["extension_hex"], "1234")
        self.assertEqual(decode(0x155, "abcd"), {})

    def test_truncation_and_sentinels(self):
        for mid in DECODERS:
            self.assertIn("truncated", decode(mid, ""))
        for mid, body in [(0x17e, "11022f0000"), (0x32f, "123456"), (0x367, "010280"),
                          (0x3cf, "340900c003493264fe34"), (0x24f, "000601")]:
            with self.subTest(mid=hex(mid)):
                self.assertIn("truncated", decode(mid, body))
        for body in ("0040", "0080", "00c0", "ffff"):
            self.assertEqual(decode(0x32f, body)["outdoor_temp"], "unavailable")
        self.assertNotIn("tank_temp_f", decode(0x23f, "000001ff"))

    def test_replay_keeps_diagnostics_and_verifies_record_integrity(self):
        record = b"F 3 000061a8 5f14 M 7d7123420582\r\n"
        end = b"END 00000000\r\n"
        decoded, = replay([record, end])
        self.assertIn("no-ack/bad-end NetworkError code=5 instance=2 priority=2", decoded)
        self.assertEqual(list(replay([end])), [])
        for lines, error in (([record], "missing END"), ([end, record], "after END"),
                             ([record, b"END 00000002\r\n"], "dropped 2"),
                             ([record.replace(b"0582", b"0583"), end], "line 1: tN4 CRC")):
            with self.subTest(error=error), self.assertRaisesRegex(ValueError, error):
                list(replay(lines))

    def test_receiver_accounting(self):
        record = b"F 3 000061a8 5f14 M 7d7123420582\r\n"
        def summary(port, counters):
            return (f"D {port}" + "".join(f" {n:08x}" for n in counters) + "\r\n").encode()
        empty = [0] * 12
        stats = [101, 4, 1, 1, 0, 0, 1, 1, 0, 12, 0x55, 0xaa]
        audit = FrameAudit(True)
        audit.record(record)
        for port in range(5):
            audit.record(summary(port, stats if port == 3 else empty))
        audit.finish(0)
        for p, counts in ((3, stats), (2, empty)):
            with self.assertRaises(ValueError):
                audit.record(summary(p, counts))
        with self.assertRaises(ValueError):
            FrameAudit(True).record(summary(3, stats))
        with self.assertRaises(ValueError):
            FrameAudit(True).finish(0)


if __name__ == "__main__":
    unittest.main()
