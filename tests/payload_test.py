"""Native tN4 application layouts. Vectors are synthetic, not site captures."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from frames import encode, replay
from payloads import DECODERS, decode_payload


def decode(mid, hex_bytes):
    return decode_payload(mid, bytes.fromhex(hex_bytes))


class PayloadTests(unittest.TestCase):
    def test_known_layouts(self):
        cases = {
            0x010: ("010002", dict(synch_cycle=1, purging=0, target_mode=2)),
            0x011: ("", {}),
            0x173: ("023c00ff", dict(tank_id=2, pump_output_pct=60, tank_temp_raw=0, target_temp_raw=255)),
            0x17f: ("2a03222122", dict(destination_type=2, destination_id=5, count=3, keys_hex="222122")),
            0x1f3: ("01020304efbe", dict(subnet_id=1, node_id=2, channel_id=3, type_id=4, io_value=0xbeef)),
            0x1f4: ("010203", dict(subnet_id=1, node_id=2, channel_id=3)),
            0x1f5: ("01020405007800ff0020083412", dict(channel_count=5, label_hex="007800ff0020",
                                                     type_id=8, io_value=0x1234)),
            0x1f6: ("0508", dict(subnet_id=5, node_id=8)),
            0x200: ("02", dict(request=0, access_open=1)),
            0x20f: ("0710", dict(group_id=7, requested_enable=0, actual_enable=1)),
            0x21f: ("0701", dict(group_id=7, enable=1)),
            0x25f: ("7634127856", dict(fan1=2, fan2=1, optimum_start=3, freeze_protect=1,
                                          duct_temp_raw=0x1234, coil_temp_raw=0x5678)),
            0x26f: ("c52a3412", dict(slab_status=5, target_raw=42, current_raw=0x1234)),
            0x287: ("02", dict(mode=2)),
            0x2ef: ("01", dict(access_lock=1, address_lock=0)),
            0x2f7: ("b61924", dict(schedule_id=2, master=1, previous_schedule_id=2,
                                      previous_master=1, events=2, current_state=1, next_state=2,
                                      duplicate_response=1, time_to_next_edge=36)),
            0x2ff: ("073412", dict(scene=7, duration=0x1234)),
            0x307: ("", {}),
            0x30f: ("15", dict(wake=1, unoccupied=0, occupied=1, sleep=0, away=1)),
            0x317: ("", {}),
            0x322: ("5580000001", dict(target_subnet=0x55, node_selector=0x01000080)),
            0x323: ("02812c34125518262d39", dict(count=2, flags=0x81, control=44, dewpoint_raw=0x1234,
                                                    internal_sensor_raw=0x55, sensor_bytes_hex="1826",
                                                    humidity_current_target=45, dehumidity_current_target=57)),
            0x327: ("", {}),
            0x347: ("c72a", dict(actuator_type=7, adjustable_speed=1, system_pump_controllable=1,
                                  motor_speed=42)),
            0x34f: ("b6649a79345678", dict(output_mode=2, cooling_logic=1, external_demand=1,
                                             adjustment_latched=1, output_limited=0, fault=1, pump_output=100,
                                             actuator_percent=26, decreasing=0, increasing=1, target_raw=0x134,
                                             supply_raw=0x256, return_raw=0x378, return_sensor_fault=1,
                                             supply_sensor_fault=0)),
            0x357: ("8fab37", dict(output_mode=3, cooling_logic=1, clear_error=1, target_raw=0x2ab,
                                    target_pump_output=55)),
            0x363: ("2bc0", dict(water_subnet=43, water_bus=3, water_control=5, cooling=1, demand=1)),
            0x36a: ("d483", dict(group=20, no_response_required=1, master=1, previous_group=3,
                                   previous_master=1)),
            0x377: ("628ab32a34120e2c785639bc9a", dict(air_group_id=2, inhibit_heat=1, inhibit_cool=1,
                     new_cycle=1, cycle_length=10, heat_demand=1, heat_target_raw=42, heat_temperature_raw=0x1234,
                     cool_enabled=0, cool_target_raw=44, cool_temperature_raw=0x5678, heat1_status=1,
                     heat2_status=2, cool1_status=3, cool2_status=0, remote_temperature_raw=0x9abc)),
            0x37f: ("8a", dict(cycle_length=10, hold_override=1)),
            0x387: ("3412", dict(additional_time=0x1234)),
            0x38f: ("027856", dict(test_mode=2, expected_duration=0x5678)),
            0x3ce: ("3412785605610c2d60", dict(supply_temp_raw=0x1234, return_temp_raw=0x5678,
                     target_temp_raw=0x105, heat_cool_mode=3, operating_on_min=1, output_limited=1)),
            0x3e7: ("432a", dict(previous_subnet=0x43, previous_node=10, locked=1)),
            0x3ef: ("", {}),
            0x3ff: ("200a0d", dict(boiler_subnet=32, mix1_subnet=10, mix2_subnet=13)),
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
            0x2d7: ("05000104000000", dict(block_id=5, target_subnet=1, target_nodes_mask="0x00000004")),
            0x2df: ("012305002c202a251e", dict(block_id=5, target_node=3, target_node_flags=1,
                        wake_f=71.6, away_f=59.0)),
            0x2e7: ("0600142830", dict(block_id=6, support_mask="0x14", occupied_raw=40, away_raw=48)),
            0x31f: ("1e631a181a03", dict(hour=12, minute=30, weekday=3, month=11, day=25,
                      year=2026, date_supported=1, dst=1, dst_mode=3)),
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
            0x3df: ("", {}),
            0x3f7: ("", {}),
        }
        for mid in (0x35f, 0x397, 0x39f, 0x3a7, 0x3af, 0x3b7, 0x3bf):
            cases[mid] = ("2181010080", dict(target_subnet=33, output_level=1, stage=1,
                                             target_nodes_mask="0x800001"))
        self.assertEqual(set(cases), DECODERS.keys())
        for mid, (body, expected) in cases.items():
            with self.subTest(mid=hex(mid)):
                fields = decode(mid, body)
                self.assertNotIn("truncated", fields)
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
        self.assertEqual(decode(0x2e7, "efbe810142"), dict(block_id=0xbeef, support_mask="0x81",
                                                           byte1_raw=1, byte8_raw=66))
        self.assertNotIn("wake_raw", decode(0x2e7, "0500101e"))
        self.assertEqual(decode(0x2d7, "0500070200000078563412")["arguments"], 0x12345678)
        self.assertIn("truncated", decode(0x2e7, "05001f01"))
        self.assertIn("truncated", decode(0x2df, "0101050028202a25"))
        fields = decode(0x31f, "002affffff")
        self.assertEqual(fields["date_supported"], 0)
        self.assertNotIn("year", fields)
        self.assertNotIn("dst_mode", fields)
        self.assertEqual(decode(0x3df, "01"), {"extension_hex": "01"})

    def test_truncation_and_sentinels(self):
        for mid in DECODERS.keys() - {0x011, 0x307, 0x317, 0x327, 0x3df, 0x3ef, 0x3f7}:
            self.assertIn("truncated", decode(mid, ""))
        for mid, body in [(0x17e, "11022f0000"), (0x32f, "123456"), (0x367, "010280"),
                          (0x3cf, "340900c003493264fe34"), (0x24f, "000601")]:
            with self.subTest(mid=hex(mid)):
                self.assertIn("truncated", decode(mid, body))
        for body in ("0040", "0080", "00c0", "ffff"):
            self.assertEqual(decode(0x32f, body)["outdoor_temp"], "unavailable")
        self.assertNotIn("tank_temp_f", decode(0x23f, "000001ff"))

    def test_variable_layout_boundaries(self):
        self.assertIn("truncated", decode(0x323, "03002a341212aaaa"))
        zero = decode(0x323, "00002a341212")
        self.assertEqual(zero["sensor_bytes_hex"], "")
        self.assertNotIn("humidity_current_target", zero)
        self.assertEqual(decode(0x323, "00002a34121239")["humidity_current_target"], 57)
        self.assertEqual(decode(0x323, "00002a341212394201")["extension_hex"], "01")
        self.assertIn("truncated", decode(0x17f, "00032233"))
        self.assertIn("truncated", decode(0x35f, "21810100"))
        self.assertEqual(decode(0x357, "40073456")["extension_hex"], "56")
        self.assertEqual(decode(0x3ce, "0000000000000000003412")["extension_hex"], "3412")
        self.assertEqual(decode(0x3ff, "000102030405060708")["extension_hex"], "08")

    def test_plant_subtype_layouts(self):
        cases = [
            ("01", {"heat_enabled": 1}), ("02", {"cool_enabled": 2}),
            ("013412", {"signal_type": 1, "output_level": 0x1234}),
            ("027856", {"signal_type": 2, "output_level": 0x5678}),
            ("c2653456f3", {"state": 2, "pump_status": 3, "stages_available_mask": 5,
                            "stages_operating_mask": 6, "input_temp_raw": 52, "output": 19}),
            ("40ba3456", {"stages_available_mask": 10, "stages_operating_mask": 11, "outlet_temp_raw": 86}),
            ("02030405060708", {"minimum_modulation_level": 4, "input_temp_raw": 6, "output": 8}),
            ("", {"plant": "modulating_chiller"}),
            ("320103", {"state": 2, "ob_config": 1, "cooling": 1, "number_stages_on": 1}),
            ("32010334127856", {"number_stages_available": 3, "input_temp_raw": 0x1234}),
            ("3201038234127856", {"pump_status": 2, "outlet_temp_raw": 0x5678}),
            ("3201038134127856", {"pump_status": 1, "input_temp_raw": 0x1234}),
            ("32030507", {"current_modulation_level": 3, "maximum_modulation_level": 7}),
            ("32030507", {"minimum_modulation_level": 5, "maximum_modulation_level": 7}),
            ("320305070234127856", {"pump_status": 2, "outlet_temp_raw": 0x5678}),
            ("320305070134127856", {"pump_status": 1, "input_temp_raw": 0x1234}),
            ("1203053412", {"state": 18, "number_stages_available": 5, "sensor_temp_raw": 0x1234}),
            ("233412", {"state": 35, "sensor_temp_raw": 0x1234}),
            ("457856", {"state": 69, "sensor_temp_raw": 0x5678}),
        ]
        for kind, (body, expected) in enumerate(cases):
            with self.subTest(kind=kind):
                prefix = f"32{kind:02x}"
                fields = decode(0x24f, prefix + body)
                self.assertEqual({k: fields[k] for k in expected}, expected)
                if body:
                    self.assertIn("truncated", decode(0x24f, prefix + body[:-2]))
        self.assertEqual(decode(0x24f, "3207aabb")["extension_hex"], "aabb")
        for kind in ("0e", "0f"):
            f = decode(0x24f, "32" + kind + "32030507020034127856")
            self.assertEqual(f["temperature_tail_hex"], "0034127856")
            self.assertNotIn("input_temp_raw", f)

    def test_snowmelt_status_fields(self):
        fields = decode(0x17e, "09070102011b02011f0700210a002affff2d3412300c00")
        self.assertEqual(fields["pair0_command_status"], 258)
        self.assertEqual((fields["pair1_operating_state"], fields["pair1_operating_substate"]), (1, 2))
        self.assertEqual(fields["pair2_electric_off_time_15s_ticks"], 7)
        self.assertEqual(fields["pair3_sensor_flags"], 10)
        self.assertEqual(fields["pair4_system_pump_contacts"], 0xffff)
        self.assertEqual(fields["pair5_supply_temperature_raw"], 0x1234)
        self.assertEqual(fields["pair6_melting_time_minutes"], 12)

    def test_support_mask_does_not_shift_positional_bytes(self):
        setup = decode(0x2e7, "0f001103aabbcc01")
        self.assertEqual(setup["block_layout"], "positional")
        self.assertEqual(setup["heating_schedule_id"], 3)
        self.assertEqual(setup["optimum_start_operation"], 1)
        self.assertNotIn("cooling_schedule_id", setup)
        self.assertEqual(setup["byte2_unsupported_raw"], 0xaa)
        self.assertEqual(decode(0x2e7, "0f00110301")["optimum_start_operation"], 1)
        partial = decode(0x2e7, "0f001103aa01")
        self.assertEqual(partial["block_layout"], "ambiguous")
        self.assertNotIn("optimum_start_operation", partial)
        self.assertEqual(partial["extension_hex"], "03aa01")
        identity = decode(0x2e7, "00007f563412ab890204")
        self.assertEqual(identity["product_number"], 0x123456)
        self.assertEqual(identity["j_number"], 0x89ab)
        self.assertEqual(identity["z_number"], 2)
        self.assertEqual(identity["max_tn4_version"], 4)
        self.assertNotIn("product_number", decode(0x2e7, "0000061234"))

    def test_schedule_event_blocks(self):
        days = "00018f9024ff06"
        for offset, name in enumerate(("wake", "unoccupied", "occupied", "sleep")):
            for base, prefix, mask in [(0x10, "", "7f"), (0x38, "ae", "ff")]:
                block = f"{base + offset:02x}00"
                for fields in (decode(0x2e7, block + mask + prefix + days),
                               decode(0x2df, "0231" + block + prefix + days)):
                    self.assertEqual(fields["event"], name)
                    self.assertEqual(fields["monday"], "00:00")
                    self.assertEqual(fields["tuesday"], "00:10")
                    self.assertEqual(fields["wednesday"], "23:50")
                    self.assertEqual(fields["thursday"], "disabled")
                    self.assertEqual(fields["friday"], "06:00")
                    self.assertNotIn("saturday", fields)
                    self.assertEqual(fields["saturday_raw"], 255)
                    self.assertEqual(fields["sunday"], "01:00")
                    if prefix:
                        self.assertEqual(fields["schedule_id"], 6)
                        self.assertEqual(fields["master_flag"], 1)
                        self.assertEqual(fields["schedule_cycling_rate"], 2)
                        self.assertEqual(fields["num_daily_events"], 2)

    def test_binary_replay(self):
        message = encode(1, bytes.fromhex("0003190000007d7123420582"))
        [row] = replay([message[:3], message[3:]])
        self.assertIn("id=0x3d7", row)
        self.assertIn("NetworkError code=5 instance=2 priority=2", row)
        [invalid] = replay([encode(1, bytes.fromhex("040200000000020900ab80"))])
        self.assertIn("bit_count=9 bits=ab80", invalid)
        for packet in (message[:-1], message[:-2] + bytes([message[-2] ^ 1, message[-1]]),
                       encode(1, bytes.fromhex("040200000000020900ab81")),
                       encode(1, bytes.fromhex("0003190000007d71234205")),
                       encode(1, b"\x80\x00")):
            with self.assertRaises(ValueError):
                list(replay([packet]))

    def test_sensor_and_humidity_configuration(self):
        for block, mask, data, expected in [
            ("0d00", "1f", "0102000000", {"onboard_sensor": 1, "sensor1": 2}),
            ("a300", "03", "0102", {"humidity_mode_setup": 1, "dehumidity_mode_setup": 2}),
            ("a400", "1f", "2021222324", {"wake_target_rh_percent": 32, "away_target_rh_percent": 36}),
            ("a500", "1f", "3031323334", {"wake_target_rh_percent": 48, "away_target_rh_percent": 52}),
        ]:
            for result in (decode(0x2e7, block + mask + data), decode(0x2df, "0018" + block + data)):
                self.assertEqual({k: result[k] for k in expected}, expected)
        limited = decode(0x2e7, "a3000202")
        self.assertNotIn("humidity_mode_setup", limited)
        self.assertEqual(limited["dehumidity_mode_setup"], 2)


if __name__ == "__main__":
    unittest.main()
