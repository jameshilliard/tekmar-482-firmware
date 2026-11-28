"""Native tN4 application layouts. Vectors are synthetic, not site captures."""

import sys
import unittest
from datetime import datetime, time, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from frames import encode, replay
from payloads import DECODERS, decode_payload
from payloads.climate import AirGroupTargets, AirStatus, AirTarget, ThermostatConfiguration, ThermostatMode as Mode
from payloads.codec import Reader
from payloads.faults import NetworkFault, SensorCircuit
from payloads.io import IoEndpoint, IoReport, IoType, io_terminal
from payloads.line import ClockReport, GatewayAccess, GroupReport, LineReportKind, NetworkLock, OutdoorTemperature, SetpointAssignment, WarmWeatherShutdown, ZoneSyncAdjustment, ZoneSyncReport, ZoneTargetMode, group_command, line_report
from payloads.plant import BoilerChillerOutput, HeatPumpOutput, MixingPresent, MixingReport, MixingUpdate, PlantKind, PlantOutput, StageBits, StageCounts
from payloads.products import block_selectors, identify_product
from payloads.queries import NativeStatusQuery, ReplyPart, SnowmeltRole, StatusRequest
from payloads.scenes import NativeScene, SceneReport, SceneResponse
from payloads.schedules import DailyEvents, LocalScheduleOverride, ScheduleBinding, ScheduleCycle, ScheduleEvent, ScheduleProfile, ScheduleStatus
from payloads.sensors import SensorRole as R, SensorSlot, ThermostatSensors
from payloads.setpoint import SetpointApplication, SetpointStatus, SetpointTargets
from payloads.snowmelt import SnowmeltAddress, SnowmeltApplication, SnowmeltOperatingState, SnowmeltPair, SnowmeltReport, WaterDetection
from payloads.thermostat import FanDuctStatus, FanSetup, HumiditySetup, HumidityStatus, SlabStatus, TargetSlot, TargetTable, ThermostatTarget as T
from payloads.water import BoilerMaximum, HeatSource, LoopAssignment, LoopKind, LoopSetupSlot, LoopTopology, StageCommand, StageDemand, StageSelection
from payloads.writes import BlockPatch, NativeField as F, WritableBlock


def decode(mid, hex_bytes):
    return decode_payload(mid, bytes.fromhex(hex_bytes))


class PayloadTests(unittest.TestCase):
    def test_stage_selection_is_a_three_byte_one_based_bitmap(self):
        ids = (0x35f, 0x397, 0x39f, 0x3a7, 0x3af, 0x3b7, 0x3bf)
        self.assertEqual(set(StageCommand), set(ids))
        wire = bytes.fromhex("ab f5 01 80 80")
        for mid in ids:
            with self.subTest(mid=mid):
                for n in range(5):
                    self.assertIn("truncated", decode_payload(mid, wire[:n]))
                self.assertEqual(decode_payload(mid, wire + b"\xca"),
                                 {"target_subnet": 0xab, "output_level": 117, "stage": 1,
                                  "target_nodes_mask": "0x808001", "extension_hex": "ca"})
        selection = StageSelection.read(Reader(wire))
        self.assertEqual(selection.nodes, (1, 16, 24))
        self.assertEqual(selection.payload, wire)
        self.assertEqual(StageSelection.read(Reader(bytes(5))).nodes, ())
        with self.assertRaises(ValueError):
            StageSelection(0, 0, 0, 1 << 24)

    def test_line_reports_and_trace_fields_use_the_same_receive_layout(self):
        cases = (
            (0x010, "070299ca", ZoneSyncReport(7, 2, 0x99), 3),
            (0x200, "f235", GatewayAccess(0xf2), 1),
            (0x20f, "1130e4", GroupReport(17, 0x30), 2),
            (0x2ef, "82ca", NetworkLock(0x82), 1),
            (0x2ff, "99b400ca", SceneReport(0x99, 180, b"\xca"), 3),
            (0x30f, "9135", WarmWeatherShutdown(0x91), 1),
            (0x31f, "2d63ffffff04e4", ClockReport(bytes.fromhex("2d63ffffff04e4")), 5),
            (0x32f, "32053305ca", OutdoorTemperature(0x532, 0x533), 2),
        )
        self.assertEqual({mid for mid, *_ in cases}, set(LineReportKind))
        for mid, wire, expected, required in cases:
            with self.subTest(mid=mid):
                data = bytes.fromhex(wire)
                self.assertEqual(line_report(mid, data), expected)
                fields = decode_payload(mid, data)
                self.assertEqual(fields["extension_hex"], data[-1:].hex())
                self.assertLessEqual(expected.fields().items(), fields.items())
                for n in range(required):
                    self.assertIsNone(line_report(mid, data[:n]))
        for mid, data in ((0x200, b"\xf3"), (0x21f, b"\x04\x01"), (0x32f, b"\x32\x05\xca"),
                          (0x011, b""), (0x307, b""), (0x317, b""), (0x327, b"")):
            self.assertIsNone(line_report(mid, data))
        self.assertTrue(GatewayAccess(0xf2).access_open)
        self.assertTrue(NetworkLock(0x82).address_lock)
        self.assertEqual(decode(0x2ef, "82")["reserved"], 32)
        self.assertEqual(decode(0x200, "f2")["reserved"], 60)
        self.assertEqual(decode(0x30f, "91")["reserved"], 4)
        flags = decode(0x20f, "03ae")
        self.assertEqual((flags["requested_enable"], flags["requested_reserved"],
                          flags["actual_enable"], flags["actual_reserved"]), (0, 7, 0, 5))
        self.assertFalse(NetworkLock(0x82).access_lock)
        self.assertEqual(WarmWeatherShutdown(0x91).active_slots, (TargetSlot.WAKE, TargetSlot.AWAY))
        self.assertIsNone(ZoneSyncReport(7, 2, 0x99).target)
        self.assertEqual(OutdoorTemperature(0x532, 0x533).fahrenheit, 48.0)
        self.assertIsNone(OutdoorTemperature(0xc000, None).fahrenheit)

    def test_setpoint_words_require_application_context(self):
        flags = decode(0x22f, "0000000000000000e40000")
        self.assertEqual((flags["aux_relay_status"], flags["requested_enable_status"],
                          flags["actual_enable_status"], flags["reserved"]), (0, 1, 2, 3))
        payload = bytes.fromhex("d7 cb 5800 4800 00c0 6c 35 ca e4")
        fields = decode_payload(0x22f, payload)
        self.assertEqual(fields["extension_hex"], "e4")
        self.assertFalse(any(name.endswith("_f") for name in fields))
        for n in range(11):
            self.assertIn("truncated", decode_payload(0x22f, payload[:n]))
        report = SetpointStatus.read(Reader(payload))
        self.assertTrue(report.cooling)
        self.assertFalse(report.demand)
        self.assertEqual(report.readings(SetpointApplication.DHW).temperature_f, 96.8)
        for app, block in zip(SetpointApplication, (0x28, 0x2a, 0x2c, 0x28, 0x2e, 0x30)):
            self.assertEqual(app.target_block, block)
            for raw in (0, 0x4000, 0x8000, 0xc000, 0xffff):
                p = SetpointStatus(0, 0, raw, raw, 0, 0, 0, 0).readings(app)
                self.assertEqual((p.target_f, p.temperature_f), (None, None))
        self.assertEqual(report.readings(SetpointApplication.FLOOR).temperature_f, -77.8)
        self.assertIsNone(SetpointStatus(0, 0, 256, 0xff, 0, 0, 0, 0).readings(SetpointApplication.DHW).target_f)
        with self.assertRaises(TypeError):
            report.readings(2)
        primary = decode(0x2e7, "2e003f 0106 0206 0306")
        secondary = decode(0x2e7, "2f000d ff ca05")
        self.assertEqual(SetpointTargets.from_reports(primary, secondary),
                         SetpointTargets(0x601, 0x602, 0x603, None, None, 0x5ca))

    def test_plant_output_keys_do_not_use_device_total_or_stage_counts_as_masks(self):
        for header in (0, 0x20, 0x2f):
            r = Reader(bytes([header]) + bytes.fromhex("04 c2 65 34 56 f3 ca"))
            p = PlantOutput.read(r)
            self.assertEqual((p.kind, p.key, p.device_total, r.remaining),
                             (PlantKind.STAGED_BOILER, (4, header >> 4), header & 15, b"\xca"))
            self.assertIsInstance(p.body, BoilerChillerOutput)
            self.assertEqual((p.body.pump_status, p.body.stages), (3, StageBits(5, 6)))
            self.assertEqual((p.body.stages.available, p.body.stages.operating), ((1, 3), (2, 3)))
        for on, avail, pct in ((1, 4, 25), (0, 2, 0), (2, 1, None), (0, 0, None)):
            p = PlantOutput.read(Reader(bytes([0, 8, 0x32, on, avail])))
            self.assertIsInstance(p.body, HeatPumpOutput)
            self.assertEqual(p.body.stages, StageCounts(on, avail))
            self.assertEqual(p.body.stages.percent_on, pct)
        for kind in (7, 255):
            r = Reader(bytes([0, kind, 0xca, 0x35]))
            p = PlantOutput.read(r)
            self.assertEqual(p.kind, PlantKind.MODULATING_CHILLER if kind == 7 else None)
            self.assertIsNone(p.body)
            self.assertEqual(r.remaining, b"\xca\x35")

    def test_observed_mixing_reports_and_requested_outputs_are_independent(self):
        present = MixingPresent.read(Reader(bytes.fromhex("e7 28")))
        self.assertEqual((present.actuator_type, present.motor_speed), (7, 40))
        mix = MixingReport.read(Reader(bytes.fromhex("fb 33 c1 ed 12 34 56")))
        self.assertEqual((mix.target_raw, mix.supply_raw, mix.return_raw, mix.sensor_flags), (0x112, 0x334, 0x256, 3))
        r = Reader(bytes.fromhex("b9 44 63 aabb"))
        update = MixingUpdate.read(r)
        self.assertEqual((update.target_raw, update.target_pump_output, r.remaining), (0x244, 99, b"\xaa\xbb"))
        self.assertNotIn("motor_speed", update.fields())
        demand = StageDemand.read(Reader(bytes.fromhex("41 df 80 2206")))
        self.assertEqual((demand.subnet, demand.stage, demand.output_on, demand.target_raw), (65, 1, True, 0x622))
        r = Reader(bytes.fromhex("4a 00 00 2206"))
        self.assertIsNone(StageDemand.read(r).target_raw)
        self.assertEqual(r.remaining, b"\x22\x06")
        self.assertEqual([BoilerMaximum(v).percent for v in (0, 100, 255)], [0, 100, None])

    def test_io_channel_values_labels_and_terminal_references(self):
        endpoint = IoEndpoint(0x43, 0xa5)
        for kind in IoType:
            r = Reader(bytes([0x43, 0xa5, 0xff, kind, 1, 0, 0xca]))
            sample = IoReport.read_value(r)
            self.assertEqual((sample.endpoint, sample.channel, sample.kind, r.remaining),
                             (endpoint, 255, kind, b"\xca"))
            self.assertEqual(sample.active, True if kind in (IoType.RELAY, IoType.DEMAND) else None)
            self.assertEqual(sample.percent, 1 if kind in (IoType.HUMIDITY, IoType.VARIABLE_SPEED_PUMP, IoType.PWM_CYCLE) else None)
        for raw in (0x100, 0xffff):
            for kind in (1, 6, 7, 0x97):
                sample = IoReport(endpoint, 6, kind, raw)
                self.assertEqual((sample.active, sample.percent), (None, None))
                self.assertEqual(sample.fields()["io_value"], raw)
        payload = bytes.fromhex("43a5 ff 09 410042ff0000 9d 3412 ca35")
        r = Reader(payload)
        sample = IoReport.read_discovery(r)
        self.assertEqual((sample.kind, sample.count, sample.label, sample.value, r.remaining),
                         (None, 9, bytes.fromhex("410042ff0000"), 0x1234, b"\xca\x35"))
        self.assertIsNone(sample.label_text)
        self.assertEqual(decode_payload(0x1f5, payload)["extension_hex"], "ca35")
        printable = decode(0x1f5, "43a5 ff 09 410042000000 01 0100")
        self.assertEqual((printable["label_text"], printable["label_hex"]), ("AB", "410042000000"))
        for mid, width in ((0x1f2, 6), (0x1f3, 6), (0x1f5, 13)):
            for n in range(width):
                self.assertIn("truncated", decode_payload(mid, payload[:n]))
        self.assertEqual([io_terminal("654", c) for c in (1, 2, 3)], ["Mod"] * 3)
        self.assertEqual((io_terminal("553", 5), io_terminal("557", 9)), ("S2", "OB"))
        self.assertIsNone(io_terminal("553", -1))
        self.assertIsNone(io_terminal("402", 0))

    def test_thermostat_operating_samples_do_not_supply_settings(self):
        r = Reader(bytes.fromhex("e9 0a06 00c0 ca"))
        fan = FanDuctStatus.read(r)
        self.assertEqual((fan.fan1, fan.fan2, fan.flags, r.remaining), (1, 2, 0xe9, b"\xca"))
        self.assertEqual(fan.fields()["coil_temp"], "unavailable")
        self.assertEqual(fan.duct_temperature_f, 69.6)
        self.assertIsNone(fan.coil_temperature_f)
        self.assertEqual(SlabStatus.read(Reader(bytes.fromhex("01 2a 0606"))).temperature_f, 69.2)
        self.assertIsNone(SlabStatus.read(Reader(bytes.fromhex("01 2a 0080"))).temperature_f)
        for status in range(256):
            self.assertEqual(SlabStatus.read(Reader(bytes([status, 42, 0, 6]))).primary, status < 2)
        r = Reader(bytes.fromhex("02 81 00 3412 22 00ff 00 39 ca35"))
        rh = HumidityStatus.read(r)
        self.assertEqual((rh.relative_percent, rh.sensor_bytes, rh.humidify_target_raw,
                          rh.dehumidify_target_raw, r.remaining), (0, b"\0\xff", 0, 57, b"\xca\x35"))
        self.assertIsNone(HumidityStatus.read(Reader(bytes.fromhex("0000ff000000"))).relative_percent)

    def test_target_tables_respect_mask_units_and_floor_maximum(self):
        for target in T:
            self.assertEqual(target.value, int(target))
            fields = decode(0x2e7, f"{target:02x}00 15 02 03 ff")
            t = TargetTable.from_report("544", target, fields)
            self.assertEqual(t.slots, (2, None, 3, None, 255))
            self.assertEqual(t.value(TargetSlot.WAKE), 1 if target.temperature else 20 if target is T.FAN else 2)
            self.assertIsNone(t.value(TargetSlot.AWAY))
        floor = TargetTable.from_report("538", T.FLOOR, decode(0x2e7, "0700 21 00 48"))
        self.assertEqual((floor.raw(TargetSlot.WAKE), floor.value(TargetSlot.WAKE), floor.maximum_raw), (0, None, 72))
        for model in ("544", "545", "546", "553"):
            t = TargetTable.from_report(model, T.FAN, decode(0x2e7, "0900 07 00 09 0b"))
            self.assertEqual((t.value(TargetSlot.WAKE), t.value(TargetSlot.UNOCCUPIED), t.value(TargetSlot.OCCUPIED)),
                             (0, 9, 11) if model == "553" else (0, 90, None))

    def test_target_bounds_match_the_representable_settings(self):
        for target, model, raw, physical, step in (
            (T.HEAT, "532", 41, 20.5, 0.5), (T.FAN, "544", 8, 80, 10),
            (T.HUMIDIFY, "553", 0, 0, 1), (T.DEHUMIDIFY, "557", 80, 80, 1),
        ):
            self.assertEqual(target.step(model), step)
            self.assertEqual(target.encode_value(physical, model), raw)
            self.assertEqual(target.physical_value(raw, model), physical)
        self.assertEqual(T.HEAT.bounds("532"), (0.5, 127))
        with self.assertRaises(ValueError):
            T.FAN.encode_value(85, "544")
        with self.assertRaises(ValueError):
            T.HUMIDIFY.encode_value(19, "553")

    def test_fan_and_humidity_enable_need_the_actual_setup_bytes(self):
        for data, enabled in (("00", None), ("01 00", None), ("01 10", True), ("02 04", True),
                              ("03 02 0c", False), ("03 01 0c", True), ("03 ff 03", False)):
            self.assertEqual(FanSetup.from_report(decode(0x2e7, "0800 " + data)).enabled, enabled)
        setup = HumiditySetup.from_report(decode(0x2e7, "a300 03 ff 00"))
        self.assertIsNone(setup.enabled(T.HUMIDIFY))
        self.assertFalse(setup.enabled(T.DEHUMIDIFY))
        setup = HumiditySetup.from_report(decode(0x2e7, "a300 02 02"))
        self.assertIsNone(setup.enabled(T.HUMIDIFY))
        self.assertTrue(setup.enabled(T.DEHUMIDIFY))

    def test_local_schedule_override_is_a_supported_raw_byte(self):
        states = ("wake", "unoccupied_4", "occupied_4", "sleep", "occupied_2",
                  "unoccupied_2", "away", "auto_4", "auto_2")
        self.assertEqual(len(LocalScheduleOverride), len(states))
        for raw, name in enumerate(states):
            fields = decode_payload(0x2e7, bytes([0x37, 0, 1, raw]))
            self.assertEqual(fields["override_state"], raw)
            self.assertEqual(fields["override_state_name"], name)
        unsupported = decode(0x2e7, "3700 00 00")
        self.assertNotIn("override_state", unsupported)
        extension = decode(0x2e7, "3700 01 ff")
        self.assertEqual(extension["override_state"], 255)
        self.assertNotIn("override_state_name", extension)
        self.assertNotIn(0x37, WritableBlock)
        self.assertNotIn(0x37, identify_product(105102).candidate_blocks)

    def test_schedule_status_is_not_a_schedule_assignment(self):
        for previous in range(8):
            for current in range(8):
                r = Reader(bytes([previous << 3 | current | 0x80, 0xf2, 0xca, 0x35]))
                status = ScheduleStatus.read(r)
                self.assertEqual(status.previous_id, previous & 3)
                self.assertEqual(status.schedule_id, current & 3)
                self.assertEqual(status.previous_master, bool(previous & 4))
                self.assertEqual(status.master, bool(current & 4))
                self.assertEqual(status.ownership_changed, current != previous)
                self.assertEqual((r.remaining, status.time_to_next_edge), (b"\x35", 0xca))
                self.assertEqual((status.current_event, status.next_event), (ScheduleEvent.OCCUPIED, ScheduleEvent.WAKE))
        for count in range(4):
            for state in range(4):
                s = ScheduleStatus(count << 6, state | (3 - state) << 2, 0xff)
                expected = (ScheduleEvent(state) if count == 2 else
                            (ScheduleEvent.UNOCCUPIED if state & 1 else ScheduleEvent.OCCUPIED) if count == 1 else None)
                self.assertEqual(s.current_event, expected)
        self.assertIn("truncated", decode(0x2f7, "a704"))
        self.assertEqual(decode(0x2f7, "480c00ca")["extension_hex"], "ca")
        self.assertNotIn("current_event", decode(0x2f7, "c000ff"))

    def test_snowmelt_endpoints_are_one_based_and_roles_are_independent(self):
        for role in SnowmeltRole:
            for n in (1, role.maximum):
                addr = SnowmeltAddress(role, n)
                self.assertEqual(SnowmeltAddress.from_report(addr.request_byte + 8), addr)
            for n in (0, role.maximum + 1):
                self.assertIsNone(SnowmeltAddress.from_report(n << 3 | role))
        self.assertIsNone(SnowmeltAddress.from_report(0x1b))
        for app in SnowmeltApplication:
            query = NativeStatusQuery.for_snowmelt_device(app, 3)
            self.assertEqual(query.keys, app.status_keys)
            self.assertEqual(query.payload[0], 0x12 if app is SnowmeltApplication.SENSOR_ONLY else 0x11)

    def test_snowmelt_pairs_retain_order_raw_words_and_faults(self):
        payload = bytes.fromhex("19 05 220006 2200c0 1b1204 9900ca 190400 35")
        r = Reader(payload)
        report = SnowmeltReport.read(r)
        self.assertEqual(report.address, SnowmeltAddress(SnowmeltRole.ZONE, 3))
        self.assertEqual([p.key for p in report.pairs], [0x22, 0x22, 0x1b, 0x99, 0x19])
        self.assertEqual(report.pairs[0].temperature_f, 68.6)
        self.assertIsNone(report.pairs[1].temperature_f)
        self.assertEqual(report.pairs[2].operating_state, SnowmeltOperatingState.MELTING)
        self.assertEqual(report.pairs[4].water_detection, WaterDetection.OPEN)
        self.assertEqual(r.remaining, b"\x35")
        fields = decode_payload(0x17e, payload)
        self.assertEqual((fields["pair2_operating_substate"], fields["pair3_value_raw"],
                          fields["pair4_water_detect_state_name"], fields["extension_hex"]),
                         (0x12, 0xca00, "open", "35"))
        self.assertEqual([SnowmeltPair(0x2a, n).pump_on for n in (0, 1, 2)], [False, True, None])
        self.assertIsNone(SnowmeltPair(0x2b, 1).pump_on)
        self.assertIsNone(SnowmeltPair(0x19, 0xab03).water_detection)
        self.assertIsNone(SnowmeltPair(0x1b, 0x1204).operating_state)
        self.assertIn("truncated", decode_payload(0x17e, payload[:-2]))

    def test_air_group_mode_is_not_the_thermostat_setting(self):
        config = decode(0x36a, "e5e3")
        self.assertEqual((config["group"], config["master"], config["reserved0"],
                          config["previous_group"], config["previous_master"], config["reserved1"]),
                         (5, 1, 1, 3, 1, 3))
        member = bytes.fromhex("a4 62 2a 0000 07 0080 aa")
        r = Reader(member)
        air = AirStatus.read(r, master=False)
        self.assertFalse(air.cool.enabled)
        self.assertTrue(air.cool.demand)
        self.assertFalse(air.cool.calling)
        self.assertEqual(air.operating_flags, (4, None, 2))
        self.assertIsNone(air.cool.temperature_f)
        self.assertIsNone(air.remote_f)
        self.assertEqual(air.cool.target_f, 69.8)
        self.assertEqual((r.remaining, decode_payload(0x36f, member)["extension_hex"]), (b"\xaa", "aa"))
        master = bytes.fromhex("65 88 07 00 2c06 13 ff 00c0 e4 b005")
        air = AirStatus.read(Reader(master), master=True)
        self.assertTrue(air.heat.calling and air.cool.calling)
        self.assertEqual((air.heat.setback, air.cool.setback), (1, 4))
        self.assertIsNone(air.cool.target_f)
        self.assertNotIn("mode_setting", decode_payload(0x377, master))
        self.assertEqual(decode(0x377, "80000000000000000000000000")["reserved0"], 1)
        for mid, data in ((0x36f, member[:7]), (0x377, master[:12])):
            self.assertIn("truncated", decode_payload(mid, data))

    def test_air_group_target_uses_master_gate_and_selected_channel_setback(self):
        book = AirGroupTargets()
        def master(g, h, c, ht=40, ct=50):
            return book.observe(AirStatus.read(Reader(bytes([g, 0, h, ht, 0, 0, c, ct, 0, 0, 0, 0, 0])), master=True))
        def member(g, f, target=42):
            return book.observe(AirStatus.read(Reader(bytes([g, f, target, 0, 0, 0, 0, 0])), master=False))
        self.assertEqual(master(16, 4, 11), AirTarget(T.COOL, 2, 50))
        self.assertEqual(master(16, 7, 11), AirTarget(T.HEAT, 1, 40))
        self.assertIsNone(master(16, 5, 9))
        self.assertIsNone(master(48, 7, 11))  # No fallback to cool if the chosen heat is inhibited.
        self.assertIsNone(member(16, 1))
        self.assertEqual(member(48, 1), AirTarget(T.COOL, 0, 42))
        self.assertEqual(master(112, 7, 11), AirTarget(T.HEAT, 1, 40))  # gate 3, not a bitmap
        self.assertEqual(master(49, 7, 11), AirTarget(T.HEAT, 1, 40))  # group 17 is not indexed
        for flags in (0, 2, 65, 129):
            self.assertIsNone(member(16, flags))
        self.assertIsNone(member(16, 1, 0))
        self.assertEqual(member(16, 33, 255), AirTarget(T.HEAT, 0, 255))
        self.assertIsNone(member(16, 33, 255).celsius)

    def test_thermostat_equipment_obeys_positional_and_compact_support_masks(self):
        product = identify_product(105102)
        self.assertIn(0x58, product.candidate_blocks)
        self.assertNotIn(0x58, identify_product(104401).candidate_blocks)
        for cooling in ("31 02 aabbcc 0103", "31 02 0103"):
            reports = {block: decode_payload(0x2e7, block.to_bytes(2, "little") + bytes.fromhex(data))
                       for block, data in ((1, "1e 8b 00 0d cb"), (4, cooling),
                                           (0x97, "02 05"), (0x58, "01 81"))}
            setup = ThermostatConfiguration.from_blocks(product, 2, reports)
            first, second = setup.stage_setup.heating_stage1, setup.stage_setup.heating_stage2
            self.assertEqual((first.terminal_raw, first.pump_delay_raw, first.pump_required), (None, 11, True))
            self.assertEqual((second.terminal_raw, second.pump_delay_raw, second.pump_required), (0, 13, False))
            self.assertEqual(setup.stage_setup.stage2_heat_source_raw, 0xcb)
            self.assertEqual((setup.cooling_stages, setup.cooling_setup.heat_cool_interlock_raw,
                              setup.cooling_setup.heat_cool_priority_raw), (2, 1, 3))
            self.assertIsNone(setup.cooling_setup.minimum_run_time_raw)
            self.assertIsNone(setup.cooling_setup.minimum_off_time_raw)
            self.assertIsNone(setup.cooling_setup.cycles_per_hour)
            self.assertEqual((setup.heat_pump_setup.available_mask, setup.enabled_heat_pumps), (None, 5))
            self.assertEqual((setup.floor_cooling_raw, setup.floor_cooling_enabled), (0x81, None))
        fan = FanSetup.from_report(decode(0x2e7, "0800 75 04 2e 13 14 15"))
        self.assertTrue(fan.enabled)
        self.assertEqual((fan.minimum_heating_coil_temp_raw, fan.maximum_cooling_coil_temp_raw,
                          fan.delay_raw, fan.purge_raw, fan.heating_purge_raw, fan.cooling_purge_raw),
                         (46, None, 19, 20, 21, None))

    def test_thermostat_equipment_monitors_keep_their_distinct_register_layouts(self):
        product = identify_product(105102)
        self.assertTrue({0x1b, *range(0x99, 0x9f), 0xa1, 0xa2, 0xa8} <= set(product.candidate_blocks))
        self.assertFalse({0x1c, 0x9f, 0xa0}.intersection(product.candidate_blocks))
        relays = decode(0x2e7, "1b00 17 01fe00 49 02 99")
        self.assertEqual((relays["heat_stage1_output"], relays["heat_stage2_output"],
                          relays["cool_stage1_output"], relays["fan1_output"]), (1, 254, 0, 2))
        self.assertNotIn("cool_stage2_output", relays)
        self.assertNotIn("fan2_output", relays)
        self.assertEqual(decode(0x2e7, "1c00 03 ffff")["heat_stage1_accumulated_time"], 65535)
        self.assertEqual(decode(0x2e7, "9900 03 0000")["w1_heat_hours"], 0)
        self.assertNotIn("w1_heat_hours", decode(0x2e7, "9900 02 ab"))
        air_filter = decode(0x2e7, "0a00 0d 81 3412")
        self.assertNotIn("filter_replacement_interval", air_filter)
        self.assertEqual(air_filter["time_since_filter_replacement"], 0x1234)
        self.assertEqual(decode(0x2e7, "a800 01 01")["onboard_humidity_sensor_enabled"], 1)

    def test_operating_policy_keeps_only_supported_raw_octets(self):
        candidates = set(identify_product(105102).candidate_blocks)
        self.assertTrue({0x0b, 0x0c, 0xa7, *range(0xa9, 0xb0)} <= candidates)
        self.assertFalse({0xa6, 0xb0}.intersection(candidates))
        for warm in ("0b00 13 02 4a aabb 39 cc", "0b00 13 02 4a 39"):
            fields = decode(0x2e7, warm)
            self.assertEqual((fields["shutdown_source"], fields["wake_warm_weather_shutdown_raw"],
                              fields["sleep_warm_weather_shutdown_raw"]), (2, 74, 57))
            self.assertNotIn("away_warm_weather_shutdown_raw", fields)
            self.assertNotIn("unoccupied_warm_weather_shutdown_raw", fields)
        self.assertEqual(decode(0x2e7, "0c00 01 20")["wake_cold_weather_shutdown_raw"], 32)
        balance = decode(0x2e7, "a900 09 00ff")
        self.assertEqual((balance["wake_balance_point_raw"], balance["sleep_balance_point_raw"]), (0, 255))
        self.assertNotIn("occupied_balance_point_raw", balance)
        self.assertNotIn("heat_pump_cycles", decode(0x2e7, "a600 00"))
        self.assertEqual(decode(0x2e7, "a700 01 09")["cool_purge_minutes"], 9)
        self.assertEqual(decode(0x2e7, "aa00 01 fe")["balance_point_schedule_id"], 254)
        self.assertEqual(decode(0x2e7, "ad00 01 05")["y_during_w2_mode"], 5)

    def test_thermostat_selector_uses_setup_and_revision_not_reported_demand(self):
        for m in Mode:
            self.assertEqual(decode_payload(0x2e7, b"\x03\x00\x01" + bytes([m]))["mode_setting_name"],
                             m.name.lower())
        self.assertEqual(decode(0x2e7, "030001aa")["mode_setting"], 0xaa)
        self.assertNotIn("mode_setting_name", decode(0x2e7, "030001aa"))
        def config(number, z=1, **kw):
            return ThermostatConfiguration.from_blocks(identify_product(number), z, {
                3: {"mode_setting": 2}, 4: kw.get("cool", {}), 0x97: kw.get("hp", {})})
        self.assertIsNone(config(105102).offers(Mode.COOL))
        self.assertTrue(config(105102, hp={"enabled_heat_pumps": 1}).offers(Mode.AUTO))
        self.assertTrue(config(105102, hp={"enabled_heat_pumps": 1}).offers(Mode.EMERGENCY))
        self.assertFalse(config(105102, hp={"enabled_heat_pumps": 0}).offers(Mode.EMERGENCY))
        self.assertTrue(config(100103, 5).offers(Mode.HEAT))
        self.assertFalse(config(100103, 6).offers(Mode.HEAT))
        self.assertIsNone(config(100103, None).offers(Mode.HEAT))
        self.assertTrue(config(100101).offers(Mode.VENT))
        self.assertFalse(config(100101).offers(Mode.BACKUP))
        self.assertEqual(Mode.AUTO.setup_dependencies(identify_product(105102)), (4, 0x97))
        self.assertEqual(Mode.EMERGENCY.setup_dependencies(identify_product(105102)), (0x97,))
        self.assertFalse(Mode.HEAT.setup_dependencies(identify_product(105102)))

    def test_water_status_optional_word_and_extensions(self):
        base = "bc07 00c0 0000 19ff f9"
        for tail, dhw, extension in (("", None, None), ("ca", None, "ca"),
                                     ("2506", 0x625, None), ("2506ca35", 0x625, "ca35")):
            fields = decode(0x3cf, base + tail)
            self.assertEqual(fields["supply_temp_f"], 113)
            self.assertEqual(fields["return_temp"], "unavailable")
            self.assertNotIn("target_temp_f", fields)
            self.assertEqual(fields["setback_status"], 1)
            self.assertEqual(fields["pump_output_pct"], 255)
            self.assertEqual(fields.get("dhw_supply_temp_raw"), dhw)
            self.assertEqual(fields.get("extension_hex"), extension)
            plant = decode(0x3ce, base + tail)
            self.assertNotIn("dhw_supply_temp_raw", plant)
            self.assertEqual(plant.get("extension_hex"), tail or None)
        self.assertIn("truncated", decode(0x3cf, "0000" * 4))

    def test_water_topology_requires_local_evidence_and_complete_slots(self):
        top = LoopTopology((0x25, 0, 0x2e))
        self.assertEqual(top.assignment_for(0x25).kind, LoopKind.BOILER)
        self.assertEqual(top.assignment_for(0x2e).mix_number, 2)
        self.assertIsNone(top.assignment_for(0))
        self.assertIsNone(LoopTopology((0x25, 0x2d)).assignment_for(0x55))
        self.assertEqual(LoopTopology((0x25, 0x2d)).assignment_for(0x2d).mix_number, 1)
        self.assertEqual(LoopTopology((0x25,), ()).assignment_for(0x25).kind, LoopKind.BOILER)
        fields = decode(0x2e7, "3400 31 29 2d 95")
        slots = LoopSetupSlot.from_block(0x34, fields)
        self.assertEqual(slots, (LoopSetupSlot(5, 0x29, None), LoopSetupSlot(7, 0x2d, HeatSource(0x95))))
        self.assertIsNone(LoopTopology((0x29,), None, slots).assignment_for(0x29))
        self.assertIsNone(LoopTopology((), (LoopSetupSlot(1, 0x25, None),), slots).assignment_for(0x25))
        self.assertEqual((fields["bus7_heat_source_kind"], fields["bus7_heat_source_loop"],
                          fields["bus7_heat_source_mode"]), ("tank", 5, 2))
        for raw, kind in ((0, "unlinked"), (3, "mix"), (0x24, "unknown"), (0xf2, "boiler")):
            source = HeatSource(raw)
            slot = LoopAssignment(2, 0x25, source)
            self.assertEqual(slot.kind.value, kind)
            self.assertEqual(LoopTopology((0x25,), (LoopSetupSlot(2, 0x25, source),)).assignment_for(0x25), slot)

    def test_clock_encodes_standard_date_even_at_day_and_year_rollover(self):
        for wall, wire in ((datetime(2007, 1, 8, 0, 30), "1e bf 10 06 07"),
                           (datetime(2024, 3, 1, 0, 1), "01 bc 11 1c 18"),
                           (datetime(2256, 1, 1, 0, 0), "00 b9 1b 1e ff")):
            report = ClockReport.from_wall_time(wall, dst=True, dst_mode=3)
            self.assertEqual(report.payload, bytes.fromhex(wire) + b"\x03")
            self.assertEqual(report.wall_datetime, wall)
            self.assertEqual(decode_payload(0x31f, report.payload)["local_datetime"], wall.isoformat(timespec="minutes"))
        winter = datetime(2012, 6, 26, 10, 27, tzinfo=timezone.utc)
        self.assertEqual(ClockReport.from_wall_time(winter).payload, bytes.fromhex("1b5205190c"))
        with self.assertRaises(ValueError):
            ClockReport.from_wall_time(datetime(2000, 1, 1), dst=True)
        with self.assertRaises(ValueError):
            ClockReport.from_wall_time(winter, dst_mode=256)

    def test_clock_does_not_invent_a_date_dst_or_a_trustworthy_weekday(self):
        report = ClockReport(bytes.fromhex("2d 63 ff ff ff 04 aa"))
        self.assertEqual(report.reported_time, time(12, 45))
        self.assertIsNone(report.daylight)
        self.assertIsNone(report.standard_date)
        self.assertIsNone(report.wall_datetime)
        data = decode_payload(0x31f, report.payload)
        self.assertNotIn("dst", data)
        self.assertEqual((data["dst_bit_raw"], data["extension_hex"], data["dst_mode"]), (1, "aa", 4))
        for raw in ("3c 01 00 00 18", "01 ff 00 00 18", "01 00 00 00 18", "00 01 01 1e 18", "00 02 00 00 18"):
            self.assertNotIn("local_datetime", decode(0x31f, raw))
        self.assertIn("truncated", decode(0x31f, "01 02 ff ff"))

    def test_group_enable_is_not_an_addressed_control_or_a_link_ack(self):
        for app in range(6):
            for group in (None, 0, 1, 12, 13, 255):
                setup = SetpointAssignment.from_fields({"application_mode": app, "setpoint_enable_address": group})
                self.assertEqual(setup.remote_group, group if app not in (2, 5) and group in (1, 12) else None)
        self.assertIsNone(SetpointAssignment.from_fields({"setpoint_enable_address": 1}).remote_group)
        self.assertEqual(group_command(12, True), b"\x0c\x01")
        for group in (0, 13):
            with self.assertRaises(ValueError):
                group_command(group, False)
        for flags in (0, 1, 16, 17, 255):
            report = GroupReport(17, flags)
            self.assertEqual((report.requested, report.actual), (bool(flags & 1), bool(flags & 16)))
            self.assertEqual(decode_payload(0x20f, bytes([17, flags]))["flags_raw"], flags)

    def test_access_status_ignores_own_query_and_zone_settings_use_three_independent_bytes(self):
        query = NativeStatusQuery.gateway_access()
        self.assertEqual((query.kind, query.payload), (0x200, b"\x01"))
        for payload in (b"\x00", b"\x02", b"\x03", b""):
            with self.assertRaises(ValueError):
                NativeStatusQuery(StatusRequest.GATEWAY_ACCESS, payload)
        for payload in (b"", b"\x01", b"\x03"):
            self.assertIsNone(query.match(0x200, 0, payload))
        for payload in (b"\x00", b"\x02", b"\xf2\xff"):
            self.assertIsNotNone(query.match(0x200, 0, payload))
        for cycle in (0, 1):
            for purge in (False, True):
                for target in ZoneTargetMode:
                    self.assertEqual(ZoneSyncAdjustment(cycle, purge, target).payload, bytes([cycle, purge, target]))
        with self.assertRaises(ValueError):
            ZoneSyncAdjustment(2, True, ZoneTargetMode.SETPOINT)

    def test_sensor_onboard_polarity_by_revision_not_external_or_setup(self):
        for number, last in ((99203, 8), (99202, 9), (99201, 9)):
            product = identify_product(number)
            setup = decode(0x2e7, "0d 00 03 01 02")
            for z, expected in ((0, R.ROOM), (last, R.ROOM), (last + 1, R.OFF), (None, None), (255, None)):
                status = decode(0x2e7, "16 00 07 00 f2 05")
                external = decode(0x2e7, "17 00 07 02 f3 05")
                reading = ThermostatSensors.from_blocks(product, z, {13: setup, 0x16: status, 0x17: external})
                onboard, floor = reading.inputs
                self.assertEqual((onboard.reported_raw, onboard.setup_raw, onboard.status_role), (0, 1, expected))
                self.assertEqual(onboard.current_f, 67.2 if expected == R.ROOM else None)
                self.assertEqual(onboard.conflict, expected == R.OFF)
                self.assertEqual((floor.slot, floor.status_role, floor.current_f), (SensorSlot.EXTERNAL_1, R.FLOOR, 67.3))
            for raw, expected in ((1, R.OFF), (2, R.FLOOR), (0xfd, None)):
                status = decode_payload(0x2e7, bytes([0x16, 0, 7, raw, 0xf2, 5]))
                self.assertEqual(ThermostatSensors.from_blocks(product, last, {0x16: status}).inputs[0].status_role, expected)

    def test_sensor_538_garbage_configuration_and_legacy_floor_display_are_distinct(self):
        for number, z, raw, word, role, display in (
            (100103, 3, 0xf3, 0, R.OFF, R.OFF), (100103, 3, 0xf4, 269, R.OFF, R.FLOOR),
            (100103, 3, 0xf3, 270, R.FLOOR, R.FLOOR), (100103, 3, 2, 0xffff, None, None),
            (100103, 3, 2, 0x4000, None, None), (100103, None, 2, 1520, None, None),
            (100103, 4, 0, 1520, R.OFF, R.FLOOR), (100103, 5, 0, 1520, R.OFF, R.OFF),
            (102302, 1, 0, 1520, R.OFF, R.FLOOR), (102302, 2, 0, 1520, R.OFF, R.OFF),
            (102302, 1, 2, 1520, R.FLOOR, R.FLOOR), (102302, 1, 0xfe, 1520, None, None),
        ):
            with self.subTest(number=number, z=z, raw=raw, word=word):
                block = decode_payload(0x2e7, bytes([0x17, 0, 0x7f, raw]) + word.to_bytes(2, "little")
                                       + bytes.fromhex("f005 f305"))
                reading = ThermostatSensors.from_blocks(identify_product(number), z, {0x17: block})
                sensor, = reading.inputs
                self.assertEqual((sensor.reported_raw, sensor.status_role, sensor.display_role), (raw, role, display))
                self.assertEqual(sensor.current_raw, word)
                self.assertEqual(sensor.current_f is not None, display == R.FLOOR)
                self.assertEqual((sensor.minimum_f, sensor.maximum_f),
                                 (67.0, 67.3) if role == R.FLOOR else (None, None))
                self.assertEqual(reading.average_floor_f is not None, role == R.FLOOR)

    def test_sensor_statistics_sparse_words_roles_and_complete_floor_mean(self):
        blocks = {13: decode(0x2e7, "0d 00 06 02 02"), 0x15: decode(0x2e7, "15 00 3f f2 05 00 40 00 00"),
                  0x17: decode(0x2e7, "17 00 67 02 f2 05 f4 05")}
        product = identify_product(107201)
        first = ThermostatSensors.from_blocks(product, 0, blocks)
        floor, missing = first.inputs
        self.assertEqual((floor.current_f, floor.minimum_f, floor.maximum_f), (67.2, None, 67.4))
        self.assertIsNone(missing.current_f)
        self.assertIsNone(first.average_floor_f)
        self.assertEqual((first.room.average_f, first.room.minimum_raw, first.room.maximum_raw), (67.2, 0x4000, 0))
        self.assertIsNone(first.room.minimum_f)
        self.assertIsNone(first.room.maximum_f)
        blocks[0x18] = decode(0x2e7, "18 00 06 f5 05")  # Current without status role: separate setup supplies it.
        self.assertEqual(ThermostatSensors.from_blocks(product, None, blocks).average_floor_f, 67.3)
        blocks[0x18] = decode(0x2e7, "18 00 03 02 f5")  # Half a word is never a temperature.
        self.assertIsNone(ThermostatSensors.from_blocks(product, None, blocks).average_floor_f)
        for raw in range(11):
            sensor, = ThermostatSensors.from_blocks(product, None, {0x17: {"sensor_configuration": raw,
                "current_value_raw": 1520, "minimum_value_raw": 0xffff, "maximum_value_raw": 0x8000}}).inputs
            self.assertEqual(sensor.status_role, R(raw))
            self.assertIsNone(sensor.minimum_f)
            self.assertIsNone(sensor.maximum_f)
            self.assertEqual(sensor.current_f is not None, raw != 0)
        with self.assertRaisesRegex(ValueError, "thermostat"):
            ThermostatSensors.from_blocks(identify_product(105801), 0, blocks)

    def test_calendar_day_mapping_and_readback_follow_identified_revision(self):
        for tektra, part, last in ((105103, 1213, 13), (104401, 1215, 2)):
            for z, monday in ((0, 1), (last, 1), (last + 1, 0)):
                profile = ScheduleProfile.from_identification(dict(product_number=tektra, j_number=part, z_number=z))
                for block in range(0x10, 0x14):
                    patch = BlockPatch.from_fields(block, {F.MONDAY: 35}).for_schedule(profile)
                    wire = patch.apply(1 << monday, bytes.fromhex("01020304050607"))
                    self.assertEqual(wire[monday], 35)
                    self.assertTrue(patch.matches(1 << monday, bytes([35])))
                    self.assertEqual(profile.wire_day(block, 6), (6 + monday) % 7)
                self.assertEqual(profile.wire_day(0x38, 0), 0)
            for z in (None, 26, 255):
                with self.assertRaisesRegex(ValueError, "revision"):
                    ScheduleProfile.from_identification(dict(product_number=tektra, z_number=z)).wire_day(0x13, 0)

    def test_protect_early_schedule_masters_and_only_known_edit_codes(self):
        zone = BlockPatch.from_fields(0xf, {F.HEATING_SCHEDULE_ID: ScheduleBinding.ZONE})
        self.assertEqual(zone.apply(0x1d, bytes.fromhex("00ec030201")).hex(), "01ec03020100")
        for mask, body in ((0x1d, bytes(4)), (0x3d, bytes(5)), (0x1d, bytes.fromhex("00ec030201cc"))):
            with self.assertRaises(ValueError):
                zone.apply(mask, body)
        for number in (99201, 99202):
            for z in (0, 10, None):
                profile = ScheduleProfile.from_identification(dict(product_number=number, z_number=z))
                for before, mask, changes in (("020103010000", 15, {F.NUM_DAILY_EVENTS: 2}),
                                             ("010103010000", 15, {F.COOLING_SCHEDULE_ID: 5}),
                                             ("010103010000", 13, {F.SCHEDULE_CYCLING_RATE: 1})):
                    with self.assertRaisesRegex(ValueError, "mastership"):
                        BlockPatch.from_fields(0xf, changes).for_schedule(profile).apply(mask, bytes.fromhex(before))
            profile = ScheduleProfile.from_identification(dict(product_number=number, z_number=11))
            patch = BlockPatch.from_fields(0xf, {F.NUM_DAILY_EVENTS: 2}).for_schedule(profile)
            self.assertEqual(patch.apply(15, bytes.fromhex("020103010000")).hex(), "020103020000")
        for block, changes in ((0xf, {F.HEATING_SCHEDULE_ID: 11}), (0xf, {F.SCHEDULE_CYCLING_RATE: 4}),
                               (0x3b, {F.NUM_DAILY_EVENTS: 3})):
            with self.assertRaises(ValueError):
                BlockPatch.from_fields(block, changes)

    def test_calendar_binding_event_count_and_day_groups(self):
        for n in range(1, 5):
            self.assertEqual(ScheduleBinding(n + 1).system_number, n)
            self.assertTrue(ScheduleBinding(n + 1).owns_table)
            self.assertEqual(ScheduleBinding(n + 5).system_number, n)
            self.assertFalse(ScheduleBinding(n + 5).owns_table)
        self.assertIsNone(ScheduleBinding.ZONE.system_number)
        self.assertFalse(ScheduleBinding.UNOCCUPIED.owns_table)
        self.assertEqual(ScheduleCycle.FIVE_TWO.days_for(6), (5, 6))
        self.assertEqual(ScheduleCycle.FIVE_ONE_ONE.days_for(3), tuple(range(5)))
        self.assertEqual(ScheduleCycle.FIVE_ONE_ONE.days_for(5), (5,))
        self.assertEqual(ScheduleCycle.DAILY.days_for(2), tuple(range(7)))
        self.assertEqual(DailyEvents.TWO.events, (ScheduleEvent.WAKE, ScheduleEvent.SLEEP))
        self.assertEqual(DailyEvents.TWO.display_event(ScheduleEvent.SLEEP), ScheduleEvent.UNOCCUPIED)
        self.assertEqual(decode(0x2e7, "0f000f 03070001")["cooling_schedule_id_name"], "member_2")
        self.assertNotIn("heating_schedule_id_name", decode(0x2e7, "0f0001 fa"))

    def test_scene_report_numbers_durations_and_future_fields(self):
        vectors = ("000000", "010000", "020000", "030000", "040000", "05b400", "06f000", "07e001")
        for scene, vector in zip(NativeScene, vectors, strict=True):
            with self.subTest(scene=scene):
                self.assertEqual(scene.payload().hex(), vector)
                self.assertEqual(SceneReport.from_payload(bytes.fromhex(vector)).scene, scene)
                self.assertEqual(decode(0x2ff, vector)["scene_number"], scene.value)
        self.assertEqual(decode(0x2ff, "ff1100cc"), dict(scene=255, duration=17, duration_minutes=17, extension_hex="cc"))
        self.assertEqual(SceneReport.from_payload(bytes.fromhex("05efbeab")), SceneReport(5, 0xbeef, b"\xab"))
        self.assertIn("truncated", decode(0x2ff, "05b4"))

    def test_scene_response_settings_are_not_applied_scenes(self):
        for scene in NativeScene:
            self.assertNotEqual(scene.fixed_response is None, scene.response_field is None)
        self.assertEqual(NativeScene.TEMPORARY_OCCUPIED.fixed_response, SceneResponse.OCCUPIED)
        # Compact report, bits 2 and 6 only. No interpretation of an unknown response.
        fields = decode(0x2e7, "0e0044 05ed")
        self.assertEqual(fields["permanent_scene_1_name"], "schedule")
        self.assertEqual(fields["temporary_scene_8"], 0xed)
        self.assertNotIn("temporary_scene_8_name", fields)
        self.assertNotIn("scenes_enabled", fields)
        for field, value in ((F.PERMANENT_SCENE_4, 6), (F.SCENES_ENABLED, 2)):
            with self.assertRaises(ValueError):
                BlockPatch.from_fields(0x0e, {field: value})
        patch = BlockPatch.from_fields(0x0e, {F.PERMANENT_SCENE_4: SceneResponse.WAKE})
        self.assertEqual(patch.apply(0x08, bytes.fromhex("aabbcc04ddeeff01")).hex(), "aabbcc00ddeeff01")
        with self.assertRaises(ValueError):
            patch.apply(0x04, bytes(8))

    def test_native_status_request_reply_vectors(self):
        cases = (
            (NativeStatusQuery(StatusRequest.ZONE_SYNC), 0x011, "", 0x010, "000102", 0, 1),
            (NativeStatusQuery.gateway_access(), 0x200, "01", 0x200, "02", 0, 1),
            (NativeStatusQuery(StatusRequest.WWSD), 0x307, "", 0x30f, "0a", 0, 1),
            (NativeStatusQuery(StatusRequest.CLOCK), 0x317, "", 0x31f, "2a61070119", 0, 1),
            (NativeStatusQuery(StatusRequest.OUTDOOR), 0x327, "", 0x32f, "0004", 0, 1),
            (NativeStatusQuery(StatusRequest.MASTER), 0x3f7, "", 0x3ff, "1122", 0, 1),
            (NativeStatusQuery.humidity(0x12, 0x80000004), 0x322, "1204000080", 0x323, "000010112233", 0, 1),
            (NativeStatusQuery.io(0x43, 0x85), 0x1f6, "4385", 0x1f5, "43850702410042434445011234", 7, 2),
            (NativeStatusQuery.io(0x43, 0x85, 7), 0x1f4, "438507", 0x1f3, "438507018000", 7, 1),
            (NativeStatusQuery.snowmelt(SnowmeltRole.SENSOR, 24, b"\x1b"), 0x17f, "ba011b", 0x17e, "c2011b0001", 0x1b, 1),
        )
        self.assertEqual(set(StatusRequest), {q.kind for q, *_ in cases})
        for query, req, payload, reply, body, item, count in cases:
            with self.subTest(query=query):
                self.assertEqual(query.kind, req)
                self.assertEqual(query.payload, bytes.fromhex(payload))
                self.assertEqual(query.match(reply, 0x12, bytes.fromhex(body)), ReplyPart(frozenset([item]), count))
                self.assertIsNone(query.match(req, 0x12, query.payload))
                empty = ReplyPart(frozenset([0]), 1) if query.kind is StatusRequest.MASTER else None
                self.assertEqual(query.match(reply, 0x12, b""), empty)
        self.assertEqual(decode(0x3ff, ""), {})
        self.assertEqual(decode(0x3ff, "00"), {"boiler_subnet": 0})
        self.assertIsNone(NativeStatusQuery.humidity(5, 4).match(0x323, 6, bytes(6)))
        self.assertIsNone(NativeStatusQuery.io(1, 2, 3).match(0x1f3, 0, bytes([1, 2, 4, 0, 0, 0])))

    def test_snowmelt_query_address_count_order_and_application(self):
        req = NativeStatusQuery.snowmelt(SnowmeltRole.ZONE, 12, bytes.fromhex("23221b22"))
        self.assertEqual(req.payload.hex(), "590423221b22")
        payload = bytes.fromhex("61 03 220002 229901 1b0103 230000")
        self.assertEqual(req.match(0x17e, 0, payload), ReplyPart(frozenset([0x22, 0x1b]), 3))
        self.assertIsNone(req.match(0x17e, 0, b"\x59" + payload[1:]))
        self.assertIsNone(req.match(0x17e, 0, b"\x62" + payload[1:]))
        self.assertIsNone(req.match(0x17e, 0, bytes.fromhex("6102230000")))
        self.assertIsNone(req.match(0x17e, 0, bytes.fromhex("6101190000")))
        for app, keys in enumerate(("19221b232d2a29", "19221b232d2a2e27", "19221b232d2a2e28", "19221b2329", "1922")):
            req = NativeStatusQuery.for_snowmelt_device(app, 1)
            self.assertEqual(req.payload[0], 2 if app == 4 else 1)
            self.assertEqual(req.keys, bytes.fromhex(keys))
        for role, number in ((SnowmeltRole.ZONE, 0), (SnowmeltRole.ZONE, 13), (SnowmeltRole.SENSOR, 25)):
            with self.assertRaises(ValueError):
                NativeStatusQuery.snowmelt(role, number, b"\x19")
        for keys in (b"", bytes(62)):
            with self.assertRaises(ValueError):
                NativeStatusQuery.for_snowmelt_device(4, 1, keys)
        with self.assertRaises(ValueError):
            NativeStatusQuery.for_snowmelt_device(255, 1)

    def test_writer_registry_and_independent_full_widths(self):
        # SW 11 BlockTypesIn.write lengths plus the model's TankUsage reset.
        groups = {
            2: (*range(0x1c, 0x22), 0x4c, 0x71, 0x72, 0x78, *range(0x99, 0xa3)),
            3: (3, 0x43, 0x44, 0x5a, 0x98),
            4: (*range(0x29, 0x32, 2),),
            5: (5, 6, 9, 0xa4, 0xa5),
            6: (7, 0x0f, 0x25),
            7: tuple(range(0x10, 0x14)),
            8: (0x0e, 0x24, 0x26, 0x27, *range(0x28, 0x32, 2), *range(0x38, 0x3c)),
        }
        self.assertEqual({i for ids in groups.values() for i in ids}, set(WritableBlock))
        for width, ids in groups.items():
            for block in map(WritableBlock, ids):
                for field in block.fields:
                    with self.subTest(block=block, field=field):
                        value = 0 if block == 0x5a else 1
                        patch = BlockPatch.from_fields(block, {field: value})
                        support = (1 << width) - 1
                        body = patch.apply(support, bytes(width))
                        self.assertEqual(len(body), width)
                        self.assertIs(patch.matches(support, body), True)
                        self.assertEqual(body != bytes(width), bool(value))

    def test_update_keeps_unsupported_bytes_selector_and_sibling_bits(self):
        cases = (
            (5, {F.OCCUPIED: 44}, 0x15, "292a2b2c2d", "292a2c2c2d"),
            (0x38, {F.MONDAY: 37, F.SCHEDULE_CYCLING_RATE: 1}, 0x83,
             "ab00010203040506", "9b25010203040506"),
            (0x44, {F.HOURS: 0}, 7, "81efbe", "810000"),
            (0x2e, {F.AWAY: 0x1234}, 0x33, "2301cafe2401", "2301cafe3412"),
            (0x0f, {F.COOLING_SCHEDULE_ID: 2}, 0x13, "0100ffabcd00", "0102ffabcd00"),
        )
        for block, edits, mask, before, expected in cases:
            with self.subTest(block=block):
                patch = BlockPatch.from_fields(block, edits)
                self.assertEqual(patch.apply(mask, bytes.fromhex(before)), bytes.fromhex(expected))
                packed = bytes(b for i, b in enumerate(bytes.fromhex(expected)) if mask & (1 << i))
                self.assertIs(patch.matches(mask, packed), True)
                self.assertIs(patch.matches(mask, bytes.fromhex(before)), False)
                self.assertIsNone(patch.matches(0, b""))

    def test_unwritable_unsafe_or_partial_edits_are_refused(self):
        for block, edits in ((0x4b, {F.HOURS: 0}), (0x5a, {F.HOURS: 2}), (5, {F.WAKE: 255}),
                             (0x10, {F.MONDAY: 145}), (0x38, {F.SCHEDULE_CYCLING_RATE: 4}),
                             (0x71, {F.SLAB_TARGET: 0}), (0x72, {F.SLAB_TARGET: 0xffff}),
                             (0x0f, {F.MASTER_FLAG: 0}), (5, {}), (5, {"wake": 0})):
            with self.subTest(block=block, fields=edits), self.assertRaises(ValueError):
                BlockPatch.from_fields(block, edits)
        for block, edits, support, body in (
            (0x0f, {F.HEATING_SCHEDULE_ID: 2}, 63, bytes.fromhex("010203040501")),
            (0x44, {F.HOURS: 0}, 3, bytes(3)),
            (0x2e, {F.MAXIMUM: 0}, 0x3f, bytes(6)),
            (5, {F.AWAY: 10}, 0x11, bytes(2)), (5, {F.WAKE: 10}, 0xff, bytes(5)),
            (5, {F.WAKE: 10}, 0x1f, bytes(6)),
        ):
            with self.subTest(block=block, support=support), self.assertRaises(ValueError):
                BlockPatch.from_fields(block, edits).apply(support, body)
        fields = decode(0x2df, "0805 0d00 01020304")
        self.assertEqual(fields["sensor3"], 4)
        self.assertNotIn("sensor4", fields)
        self.assertNotIn("truncated", fields)

    def test_read_selectors_use_wire_ids_and_known_availability(self):
        for product, block, subnet, masks, expected in (
            (101103, 0x38, 8, {}, (0, 1, 2, 3)),
            (103301, 0x44, 8, {}, (0, 129)),
            (104106, 0x44, 8, {}, (0,)),
            (102002, 0x44, 8, {}, (0, 1)),
            (103301, 0x43, 8, {0x35: 0}, (0,)),
            (102002, 0x43, 8, {0x35: 0x84}, (2, 7)),
            (102002, 0x45, 8, {0x35: 0}, ()),
            (104106, 0x5a, 8, {0x5b: 8}, (3,)),
            (104106, 0x48, 0x23, {}, (2,)),
            (104106, 0x49, 0x21, {}, (0,)),
            (104106, 0x49, 0x20, {}, ()),
            (105801, 0x48, 0x23, {}, ()),
            (0, 0x49, 0x23, {}, ()),
            (0, 0x7a, 0, {}, (0,)),
            (0, 0x01, 0, {}, (None,)),
        ):
            with self.subTest(product=product, block=block):
                self.assertEqual(block_selectors(block, identify_product(product), subnet, masks), expected)

    def test_catalog_uses_product_and_j_number_not_address_or_order(self):
        for number, j, model in (
            (101901, 1159, "346"), (101901, 1145, "481"),
            (10599181, 1118, "541"), (10599181, 1116, "542"),
            (103301, 1191, "402"), (104103, 1206, "402"),
            (105801, 1226, "654"),
        ):
            with self.subTest(number=number, j=j):
                fields = decode_payload(0x2e7, b"\0\0\x1f" + number.to_bytes(3, "little") + j.to_bytes(2, "little"))
                self.assertEqual(fields["model"], model)
        for number, j in ((101901, None), (10599181, 0), (101901, 123), (0x123456, 1206)):
            self.assertIsNone(identify_product(number, j))
        self.assertNotIn("model", decode(0x2e7, "0000 18 b604"))
        self.assertNotIn("model", decode_payload(0x2e7, b"\0\0\x07" + (101901).to_bytes(3, "little")))
        self.assertIn(0x22, identify_product(103301).candidate_blocks)
        self.assertIn(0xcd, identify_product(105801).candidate_blocks)
        self.assertEqual(identify_product(108402).candidate_blocks, (0,))
        for number, model in ((108402, "671"), (101103, "033")):
            # Missing J permits a unique product match; supported J=0 is corrupt.
            data = number.to_bytes(3, "little")
            self.assertEqual(decode_payload(0x2e7, b"\0\0\x07" + data)["model"], model)
            self.assertNotIn("model", decode_payload(0x2e7, b"\0\0\x1f" + data + bytes(2)))

    def test_legacy_identification_fixes_require_the_matching_wire_product(self):
        for number, raw_j, j, model in (
            (10599181, 24068, 1118, "541"), (15041281, 24068, 1118, "541"),
            (10599181, 23556, 1116, "542"), (15106817, 23556, 1116, "542"),
            (3006346, 29956, 1141, "161"), (3071882, 30212, 1142, "162"),
        ):
            data = number.to_bytes(3, "little") + raw_j.to_bytes(2, "little")
            fields = decode_payload(0x2e7, b"\0\0\x1f" + data)
            self.assertEqual((fields["product_number"], fields["j_number"]), (number, raw_j))
            self.assertEqual((fields["corrected_j_number"], fields["model"]), (j, model))
        fields = decode(0x2e7, "0000 3f 000000 8904 03")
        self.assertEqual((fields["product_number"], fields["corrected_product_number"], fields["model"]),
                         (0, 100102, "537"))
        for text in ("0000 18 8904", "0000 07 000000", "0000 1f 000000 045e",
                     "0000 1f 123456 045e"):
            fields = decode(0x2e7, text)
            self.assertFalse(set(fields) & {"model", "corrected_product_number", "corrected_j_number"})

    def test_registered_addressed_blocks_have_named_report_fields(self):
        registered = {
            *range(0x51), 0x53, 0x54, 0x55, 0x58, 0x5a, 0x5b, 0x62,
            *range(0x71, 0x7e), *range(0x83, 0xb1), *range(0xc0, 0xd3),
        }
        self.assertEqual(len(registered), 166)
        for block in (*sorted(registered), 0x51, 0x7e, 0xabcd):
            with self.subTest(block=hex(block)):
                data = block.to_bytes(2, "little") + bytes.fromhex("ff 0102030405060708")
                fields = decode_payload(0x2e7, data)
                self.assertNotIn("truncated", fields)
                semantic = {k for k in fields if not k.startswith("byte")} - {"block_id", "support_mask"}
                self.assertEqual(bool(semantic), block in registered)

    def test_selected_plant_registers_preserve_selectors_and_word_boundaries(self):
        for block, selector, field in (
            (0x43, "boiler_number", "boiler_hours"), (0x44, "pump_number", "pump_hours"),
            (0x4d, "mix_pump_id", "mix_pump_hours"), (0x5a, "tank_id", "tank_pump_hours"),
            (0x98, "heat_pump_number", "heat_pump_hours"),
        ):
            with self.subTest(block=hex(block)):
                bid = f"{block:02x}00"
                for mid, packet in ((0x2e7, bid + "07"), (0x2df, "0c03" + bid)):
                    fields = decode(mid, packet + "813412ee")
                    self.assertEqual((fields[selector], fields[field], fields["extension_hex"]),
                                     (129, 0x1234, "ee"))
                for payload in ("06 3412", "06 ee3412"):
                    hours_only = decode(0x2e7, bid + payload)
                    self.assertNotIn(selector, hours_only)
                    self.assertEqual(hours_only[field], 0x1234)
                partial = decode(0x2e7, bid + "05 8107")
                self.assertEqual(partial[selector], 129)
                self.assertEqual(partial["byte3_raw"], 7)
                self.assertNotIn(field, partial)
        pid = decode(0x2e7, "4600ff 72 3412 7856 bc9a de")
        self.assertEqual((pid["heating_source_id"], pid["proportional_coefficient"],
                          pid["integral_coefficient"], pid["derivative_coefficient"], pid["inhibit_time"]),
                         (0x72, 0x1234, 0x5678, 0x9abc, 0xde))

    def test_setup_flags_masks_and_units(self):
        self.assertEqual(decode(0x2e7, "0800 02 fe")["fan_ventilation_reserved"], 15)
        self.assertEqual(decode(0x2e7, "8300 01 a7")["water_temperature_reserved"], 20)
        fan = decode(0x2e7, "0800 03 e439")
        self.assertEqual((fan["fan_operation_type"], fan["fan_heat_stage1"], fan["fan_heat_stage2"],
                          fan["fan_cool_stage1"], fan["fan_cool_stage2"], fan["ventilation_mode"]),
                         (0, 1, 2, 3, 1, 2))
        self.assertEqual(fan["fan_ventilation_flags"], 0x39)
        pump = decode(0x2e7, "0200 01 9a")
        self.assertEqual((pump["pump_required"], pump["pump_delay"]), (1, 26))
        self.assertEqual(decode(0x2e7, "0100 08 2d")["stage2_pump_required"], 0)
        self.assertEqual(decode(0x2e7, "8300 01 86")["tank_flag"], 1)
        prefs = decode(0x2e7, "1400 24 1122")
        self.assertEqual((prefs["backlight"], prefs["exercise_enable"]), (17, 34))
        for data in ("2f 0102030456", "2f 01020304ee56"):
            plant = decode(0x2e7, "4100" + data)
            self.assertEqual(plant["enabled_dhw_pumps"], 4)
            self.assertEqual(plant["primary_pump_purge_time"], 0x56)
            self.assertNotIn("primary_pump_rotation_time", plant)
        busses = decode(0x2e7, "3400 30 2d15")
        self.assertEqual((busses["bus7_water_bus"], busses["bus7_water_control"], busses["bus7_heat_source_id"]),
                         (5, 5, 0x15))
        self.assertEqual(decode(0x2e7, "4e00 03 fa04")["outdoor_low_f"], 42.4)
        self.assertEqual(decode(0x2e7, "4f00 03 0080")["outdoor_high"], "unavailable")
        self.assertEqual(decode(0x2e7, "3700 01 08")["override_state_name"], "auto_2")
        self.assertNotIn("override_state_name", decode(0x2e7, "3700 01 09"))
        count = decode(0x2e7, "6200 0f aa55ff00")
        self.assertEqual(count["received_message_count"], 0xff55aa)
        self.assertNotIn("received_message_count", decode(0x2e7, "6200 0b aa5500"))
        self.assertEqual(decode(0x2df, "0002 9100 3c3412")["minimum_compressor_run_time"], 0x1234)
        self.assertEqual(decode(0x2e7, "a200 03 3492")["dehumidification_hours"], 0x9234)
        dhw = decode(0x2e7, "2400 13 0399aa8832")
        self.assertEqual((dhw["mode"], dhw["wake_exchange_target_raw"], dhw["sleep_exchange_target_raw"]),
                         (3, 0x99, 0x32))
        self.assertNotIn("occupied_exchange_target_raw", dhw)

    def test_setpoint_control_tables_preserve_word_pairs_and_extensions(self):
        self.assertEqual(decode(0x2e7, "260005 04 03")["run_mode_configuration"], 3)
        for block, app in zip(range(0x28, 0x32, 2), ("setpoint","hot_tub","dhw","floor","pool")):
            with self.subTest(app=app):
                packed = decode(0x2e7, f"{block:02x}00 33 4906 4505")
                self.assertEqual(packed["control_application"], app)
                self.assertEqual(packed["wake_occupied_raw"], 0x649)
                self.assertEqual(packed["away_raw"], 0x545)
                self.assertNotIn("unoccupied_sleep_raw", packed)
                partial = decode(0x2e7, f"{block:02x}00 01 11")
                self.assertEqual(partial["byte1_raw"], 17)
                self.assertNotIn("wake_occupied_raw", partial)
                full = decode(0x2df, f"0001 {block:02x}00 4906 1206 1004 2108 de")
                self.assertEqual(full["maximum_raw"], 0x821)
                self.assertEqual(full["extension_hex"], "de")
                second = decode(0x2df, f"0001 {block+1:02x}00 4906 ffff")
                self.assertEqual(second["wake_raw"], 0x649)
                self.assertEqual(second["sleep"], "unavailable")
        floor = decode(0x2df, "0001 2e00 4906 1206 1004")
        self.assertNotIn("maximum_raw", floor)
        self.assertEqual(floor["away_raw"], 0x410)
        self.assertIn("truncated", decode(0x2df, "0001 3000 4906 1206 1004"))

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
                       operating_mode=2, stat_priority=3, target_temp_raw=1700,
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
        for mid in DECODERS.keys() - {0x011, 0x307, 0x317, 0x327, 0x3df, 0x3ef, 0x3f7, 0x3ff}:
            with self.subTest(mid=hex(mid)):
                self.assertIn("truncated", decode(mid, ""))
        for mid, body in [(0x17e, "11022f0000"), (0x32f, "123456"), (0x367, "010280"),
                          (0x3cf, "340900c003493264"), (0x24f, "000601")]:
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
        self.assertEqual(fields["pair1_operating_state_name"], "off")
        self.assertNotIn("pair0_operating_state_name", decode(0x17e, "0901 1b00ff"))
        self.assertEqual(fields["pair2_electric_off_time_15s_ticks"], 7)
        self.assertEqual(fields["pair3_sensor_flags"], 10)
        self.assertEqual(fields["pair4_system_pump_contacts"], 0xffff)
        self.assertEqual(fields["pair5_supply_temperature_raw"], 0x1234)
        self.assertEqual(fields["pair6_melting_time_minutes"], 12)
        temperatures = decode(0x17e, "0903 2c3305 2d0040 2e0000")
        self.assertEqual(temperatures["pair0_return_temperature_f"], 48.1)
        self.assertEqual(temperatures["pair1_supply_temperature"], "unavailable")
        self.assertEqual(temperatures["pair2_supply_target_raw"], 0)
        self.assertNotIn("pair2_supply_target_f", temperatures)

    def test_snowmelt_configuration_reports_and_updates(self):
        vectors = {
            0x71: ("9c04", {"melt_setpoint_raw": 1180, "melt_setpoint_f": 33.0}),
            0x72: ("0000", {"idle_setpoint_raw": 0, "idle_setpoint": "off"}),
            0x73: ("04", {"water_sensitivity": 4}),
            0x74: ("2c01", {"melt_run_time_minutes": 300}),
            0x75: ("3c00", {"additional_melt_time_minutes": 60}),
            0x76: ("7f05", {"cold_weather_cutoff_raw": 0x57f}),
            0x77: ("fffe", {"warm_weather_shutdown_raw": 0xfeff}),
            0x78: ("1a04", {"storm_setpoint_raw": 1050, "storm_setpoint_f": 20.0}),
            0x79: ("0001", {"storm_time_minutes": 256}),
            0x7a: ("0005", {"sensor_id": 0, "snow_ice_sensor_configuration": 5}),
            0x7b: ("0207", {"sensor_id": 2, "slab_sensor_configuration": 7}),
            0x7c: ("ff", {"slab_protection": 255}),
            0x7d: ("1800", {"maximum_melt_time_hours": 24}),
            0xc0: ("08", {"zone_id": 8}),
            0xc1: ("00", {"sensor_id": 0}),
            0xc2: ("05", {"melt_group_id": 5}),
            0xc3: ("0c", {"storm_group_id": 12}),
            0xc4: ("07", {"tracked_zone_id": 7}),
            0xc5: ("0204", {"sensor_id": 2, "melt_start_type": 4}),
            0xc6: ("013412", {"sensor_id": 1, "slab_high_raw": 0x1234}),
            0xc7: ("027856", {"sensor_id": 2, "slab_low_raw": 0x5678}),
            0xc8: ("039a78", {"sensor_id": 3, "supply_high_raw": 0x789a}),
            0xc9: ("04bc9a", {"sensor_id": 4, "supply_low_raw": 0x9abc}),
            0xca: ("05debc", {"sensor_id": 5, "return_high_raw": 0xbcde}),
            0xcb: ("06f0de", {"sensor_id": 6, "return_low_raw": 0xdef0}),
            0xcc: ("072c01", {"relay_id": 7, "heat_relay_hours": 300}),
            0xcd: ("080100", {"relay_id": 8, "heat_relay_cycles": 1}),
            0xce: ("0900ff", {"sensor_id": 9, "wet_level_raw": 0xff00}),
            0xcf: ("0a1234", {"sensor_id": 10, "dry_level_raw": 0x3412}),
            0xd0: ("0403", {"application_mode": 4, "application_mode_name": "sensor_only", "application_type": 3}),
            0xd1: ("078967", {"control_id": 7, "boiler_minimum_raw": 0x6789}),
            0xd2: ("021a06", {"control_id": 2, "boiler_maximum_raw": 0x61a}),
        }
        for block, (hex_data, expected) in vectors.items():
            with self.subTest(block=hex(block)):
                prefix = f"{block:02x}00"
                mask = (1 << len(bytes.fromhex(hex_data))) - 1
                for mid, packet in ((0x2e7, prefix + f"{mask:02x}"),
                                    (0x2df, "1223" + prefix)):
                    fields = decode(mid, packet + hex_data + "abcd")
                    self.assertEqual({k: fields[k] for k in expected}, expected)
                    self.assertEqual(fields["extension_hex"], "abcd")
                    self.assertIn("truncated", decode(mid, packet + hex_data[:-2]))
        self.assertNotIn("melt_setpoint", decode(0x2e7, "7100030000"))
        self.assertEqual(decode(0x2e7, "7800030000")["storm_setpoint"], "off")
        self.assertEqual(decode(0x2e7, "720003ffff")["idle_setpoint"], "unavailable")
        self.assertNotIn("application_mode_name", decode(0x2e7, "d00003ff00"))

    def test_indexed_snowmelt_blocks_preserve_selector_and_partial_words(self):
        for block, selector in ((0x7a, "sensor_id"), (0x7b, "sensor_id"),
                                (0xc5, "sensor_id"), (0xcb, "sensor_id"),
                                (0xcc, "relay_id"), (0xcd, "relay_id"),
                                (0xce, "sensor_id"), (0xd1, "control_id")):
            req = f"{block:02x}00 12 04000000"
            self.assertNotIn(selector, decode(0x2d7, req))
            self.assertEqual(decode(0x2d7, req + "00000000")[selector], 0)
            fields = decode(0x2d7, req + "78563412 99")
            self.assertEqual(fields[selector], 0x12345678)
            self.assertEqual(fields["extension_hex"], "99")
        self.assertNotIn("sensor_id", decode(0x2d7, "c100120400000000000000"))
        for data in ("c60006 3412", "c60006 ff3412"):
            fields = decode(0x2e7, data)
            self.assertEqual(fields["slab_high_raw"], 0x1234)
            self.assertNotIn("sensor_id", fields)
        incomplete = decode(0x2e7, "c60005 11ab")
        self.assertEqual(incomplete["sensor_id"], 17)
        self.assertEqual(incomplete["byte3_raw"], 0xab)
        self.assertNotIn("slab_high_raw", incomplete)
        extended = decode(0x2e7, "c6000f 0034129a")
        self.assertEqual(extended["byte4_raw"], 0x9a)
        no_value = decode(0x2e7, "720000 cc")
        self.assertEqual(no_value["extension_hex"], "cc")
        self.assertNotIn("idle_setpoint", no_value)

    def test_secondary_block_requests_name_the_argument_not_a_report(self):
        for ids, selector in (
            (range(0x38, 0x3c), "schedule_id"), ((0x3c, 0x3d, 0x43, 0x45), "boiler_number"),
            ((0x44,), "pump_number"), ((0x46,), "heating_source_id"), ((0x47,), "cooling_source_id"),
            ((0x48, 0x49), "port_id"), ((0x4d,), "mix_pump_id"), ((0x5a,), "tank_id"),
            (range(0x8b, 0x93), "heat_pump_source_id"), ((0x98,), "heat_pump_number"),
        ):
            for block in ids:
                with self.subTest(block=hex(block)):
                    prefix = f"{block:02x}00 12 00040000"
                    self.assertNotIn(selector, decode(0x2d7, prefix))
                    self.assertEqual(decode(0x2d7, prefix + "03000000")[selector], 3)

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

    def test_network_fault_word_and_code_specific_descriptors(self):
        fault = NetworkFault.read(Reader(bytes.fromhex("8fe1")))
        self.assertEqual((fault.raw, fault.code, fault.instance, fault.priority), (0xe18f, 0x8f, 33, 3))
        self.assertEqual(fault.circuit, SensorCircuit.OPEN)
        self.assertIsNone(fault.detail)
        self.assertEqual(NetworkFault(0x108f).circuit, SensorCircuit.SHORT)
        self.assertIsNone(NetworkFault(0x2194).circuit)
        for word, expected in [(0x0809, "duplicate snowmelt zone"), (0x0909, "duplicate snow/ice sensor"),
                               (0x2107, "duplicate master for schedule 1"),
                               (0x0308, "missing master for cool group 3"),
                               (0x3104, "missing device 17"), (0x8a0b, "no flow proof for pump 2"),
                               (0x2384, "plant device 4"), (0x2382, "cooling duct"),
                               (0x2390, "cool group 3"), (0x1199, "remote sensor 18")]:
            self.assertEqual(NetworkFault(word).detail, expected)
        self.assertTrue(NetworkFault(0xfe00).is_clear)
        unknown = NetworkFault(0xd1fd)
        self.assertEqual(unknown.priority, 3)
        self.assertIsNone(unknown.detail)
        self.assertEqual(unknown.description, "Unknown Error")
        self.assertFalse(unknown.is_clear)

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
