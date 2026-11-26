"""Native tN4 payloads, independent of the 482's host-side tRPC methods."""

from collections.abc import Callable

Fields = dict[str, str | int | float]
SETBACK = ("wake", "unoccupied_4", "occupied_4", "sleep", "occupied_2", "unoccupied_2", "away", "unknown")


class ShortPayload(ValueError):
    pass


class Reader:
    def __init__(self, data: bytes):
        self.data = data
        self.pos = 0

    def take(self, n: int) -> bytes:
        end = self.pos + n
        if end > len(self.data):
            raise ShortPayload(f"need_{end}_bytes_have_{len(self.data)}")
        field = self.data[self.pos:end]
        self.pos = end
        return field

    def byte(self) -> int:
        return self.take(1)[0]

    def word(self) -> int:
        return int.from_bytes(self.take(2), "little")

    @property
    def remaining(self) -> bytes:
        return self.data[self.pos:]


def temp(name: str, raw: int, *, deg_e: bool = False) -> Fields:
    fields: Fields = {name + "_raw": raw}
    # Preserve the special value rather than turning a missing sensor into a reading.
    if raw in ((0xff,) if deg_e else (0x4000, 0x8000, 0xc000, 0xffff)):
        fields[name] = "unavailable"
    elif raw != 0:
        fields[name + "_f"] = round(raw * 0.9 + 32 if deg_e else (raw - 850) / 10, 1)
    return fields


def subnet(value: int) -> Fields:
    return {"water_subnet": value, "water_bus": value & 7, "water_control": value >> 3}


def snowmelt(r: Reader) -> Fields:
    address, count = r.take(2)
    fields: Fields = {"source_type": address & 7, "source_id": address >> 3, "pairs": count}
    for index in range(count):
        key, value = r.byte(), r.word()
        prefix = f"pair{index}_"
        fields[prefix + "key"] = f"0x{key:02x}"
        if key in (34, 35, 36):
            name = {34: "slab_temperature", 35: "slab_setpoint", 36: "edge_temperature"}[key]
            fields.update(temp(prefix + name, value))
        elif key == 47:
            fields[prefix + "keep_alive"] = value
        else:
            fields[prefix + "value_raw"] = value
    return fields


def io_change(r: Reader) -> Fields:
    sub, node, channel, kind = r.take(4)
    return {"subnet_id": sub, "node_id": node, "channel_id": channel,
            "type_id": kind, "io_value": r.word()}


def status_flags(value: int) -> Fields:
    setback = (value >> 1) & 7
    return {"enabled": value & 1, "setback_state": setback, "setback_name": SETBACK[setback],
            "optimum_start": (value >> 4) & 3, "priority": value >> 6}


def setpoint(r: Reader) -> Fields:
    flags = status_flags(r.byte())
    b = r.byte()
    fields = flags | {"actuator_status": b & 3, "demand_status": (b >> 2) & 1,
                      "heat_cool_mode": (b >> 3) & 1, "operating_mode": (b >> 4) & 3,
                      "stat_priority": b >> 6}
    for name in ("target_temp", "temp1", "temp2"):
        fields.update(temp(name, r.word()))
    aux, excuse1, excuse2 = r.take(3)
    return fields | {"aux_relay_status": aux & 3, "requested_enable_status": (aux >> 2) & 3,
                     "actual_enable_status": (aux >> 4) & 3, "reserved": aux >> 6,
                     "status_excuse1": excuse1, "status_excuse2": excuse2}


def dhw(r: Reader) -> Fields:
    fields = status_flags(r.byte())
    b, target, tank = r.take(3)
    return (fields | {"actuator_status": b & 3, "demand_status": (b >> 2) & 1,
                      "heat_cool_mode": (b >> 3) & 3, "operating_mode": b >> 5}
            | temp("target_temp", target, deg_e=True) | temp("tank_temp", tank, deg_e=True))


def plant(r: Reader) -> Fields:
    device, kind = r.take(2)
    fields: Fields = {"device_total": device & 15, "device_id": device >> 4, "device_type": kind}
    if kind == 6:  # Modulating boiler, the subtype present in the capture.
        b, current, minimum, maximum, inlet, outlet, output = r.take(7)
        fields.update(plant="modulating_boiler", state=b & 15, reserved=(b >> 4) & 3,
                      pump_status=b >> 6, current_modulation_level=current,
                      minimum_modulation_level=minimum, maximum_modulation_level=maximum,
                      input_temp_raw=inlet, outlet_temp_raw=outlet, output=output & 31,
                      return_protection=output >> 5)
    return fields


def outdoor(r: Reader) -> Fields:
    current = r.word()
    fields = temp("outdoor_temp", current)
    if r.remaining:
        # J1196J 0x3dd8 emits the word twice; 0x3e38 reads the first word only.
        second = r.word()
        fields.update(second_word_raw=second, matches_first=int(second == current))
    return fields


def stage(r: Reader) -> Fields:
    fields = subnet(r.byte())
    b, p = r.take(2)
    fields.update(stage=b & 1, override_status=(b >> 1) & 3, first_report=(b >> 3) & 1,
                  setpoint_control=(b >> 4) & 1, dhw_control=(b >> 5) & 1,
                  priority_level=b >> 6, pump_delay=p & 63, pump_not_required=(p >> 6) & 1,
                  output_on=p >> 7)
    if p & 0x80:
        fields.update(temp("target_temp", r.word()))
    return fields


def air_member(r: Reader) -> Fields:
    a, b, target = r.take(3)
    actual, outputs, remote = r.word(), r.byte(), r.word()
    return ({"air_group_id": a & 31, "mode": "cool" if a & 32 else "heat",
             "request_for_master_report": (a >> 6) & 1, "reserved0": a >> 7,
             "heat_cool_enabled": b & 1, "heat_cool_demand": (b >> 1) & 1,
             "schedule_status": (b >> 2) & 7, "schedule_name": SETBACK[(b >> 2) & 7],
             "optimum_start": (b >> 5) & 3,
             "weather_shutdown": b >> 7, "output_1_status": outputs & 3,
             "output_2_status": (outputs >> 2) & 3, "reserved5": outputs >> 4}
            | temp("target", target, deg_e=True) | temp("temperature", actual)
            | temp("remote_temperature", remote))


def master(r: Reader) -> Fields:
    supply, ret, mode_target = r.word(), r.word(), r.word()
    loop, pump, b = r.take(3)
    fields = (temp("supply_temp", supply) | temp("return_temp", ret)
              | temp("target_temp", mode_target & 0x1fff))
    mode = mode_target >> 13
    modes = ("neither", "heating", "heating_cooling_disallowed", "cooling", "cooling_heating_disallowed")
    fields.update(heat_cool_mode=mode, mode_name=modes[mode] if mode < len(modes) else "unknown",
                  loop_output_pct=loop, pump_output_pct=pump,
                  setback_status=b & 7, weather_shutdown=(b >> 3) & 1,
                  operating_on_max=(b >> 4) & 1, operating_on_min=(b >> 5) & 1,
                  output_limited=(b >> 6) & 1, reserved=b >> 7)
    if r.remaining:
        fields.update(temp("dhw_supply_temp", r.word()))
    return fields


def network_error(r: Reader) -> Fields:
    code, value = r.take(2)
    return {"code": code, "instance": value & 63, "priority": value >> 6}


DECODERS: dict[int, tuple[str, Callable[[Reader], Fields]]] = {
    0x17e: ("SnowmeltZoneStatus", snowmelt),
    0x1f2: ("IOChangeReport", io_change),
    0x22f: ("SetpointStatus", setpoint),
    0x23f: ("DHWStatus", dhw),
    0x24f: ("PlantOutputStatus", plant),
    0x32f: ("OutdoorTemperatureReport", outdoor),
    0x337: ("MaximumBoilerPercent", lambda r: {"max_value": r.byte()}),
    0x33f: ("CycleLengthRequest", lambda r: {"air_group_id": r.byte(), "requested_length": r.byte()}),
    0x367: ("StageHeatDemand", stage),
    0x36f: ("AirGroupMemberStatus", air_member),
    0x3c7: ("RelayStatusHeartbeat", lambda r: subnet(r.byte())),
    0x3cf: ("MasterStatus", master),
    0x3d7: ("NetworkError", network_error),
}


def decode_payload(message: int, data: bytes) -> Fields:
    if message not in DECODERS:
        return {}
    reader = Reader(data)
    try:
        fields = DECODERS[message][1](reader)
    except ShortPayload as exc:
        return {"truncated": str(exc)}
    if reader.remaining:
        fields["extension_hex"] = reader.remaining.hex()
    return fields


def details(message: int, payload: bytes) -> str:
    if message not in DECODERS:
        return ""
    fields = decode_payload(message, payload)
    return " ".join([DECODERS[message][0], *(f"{key}={v}" for key, v in fields.items())])
