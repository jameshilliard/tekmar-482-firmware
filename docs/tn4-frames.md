# Native tN4 frame reference

Use this reference to interpret bytes from a 482 running the custom bridge.
The [UART interface](interfaces.md#stream-decoded-frames) transports these
frames. The stock 482's TPCK/tRPC methods are a different protocol: for
example, native `0x31f` is a remote clock report, not a request to read the
482's clock.

The catalog below covers the 62 message IDs registered in the 483 SW 11
`message_types.rb` and `message_types_in.rb` registries. The [host
decoder](../tools/payloads.py) recognizes all 62. A registration is evidence
of a message layout, not evidence that a particular model sends it. The
bridge captures other IDs and preserves their payloads. This is not a vendor
specification or an assertion that no other tN4 IDs exist.

## From a bus edge to a frame

At the MCU receive pin, idle is low and dominant is high. A nominal bit is
2,500 microseconds (400 bit/s). After seven recessive bits, a dominant
start of frame (SOF) begins a candidate. The receiver samples near the
center of each bit and re-centers on captured transitions.

```text
SOF | 32-bit header | 0..63 payload bytes | CRC-15 | delimiter | ACK | delimiter | EOF
 1        MSB first          MSB first      MSB first      0      0/1       0        7 x 0
```

Stuffing spans SOF, header, payload and CRC: a complementary bit interrupts
a run of five equal bits when another bit in the stuffed region follows.
Delimiter, ACK and EOF are not stuffed. For the CRC, start at zero, feed the
SOF one, then the **unstuffed** header and payload, MSB first, with polynomial
`0x4599` (15 bits, no final XOR). Equivalently start at `0x4599` when the
input starts at header byte 0. Do not include timestamp, port, delimiter or
ACK. See the [receiver](../src/tn4.rs) and [transmitter](../src/tn4/tx.rs).

Interpret the four header bytes as one **big-endian** integer:

| Bits | Meaning |
| --- | --- |
| 31:30 | Priority |
| 29:20 | Native message ID |
| 19:12 | Source subnet |
| 11:6 | Source node |
| 5:0 | Payload length in bytes; excludes header, CRC and trailer |

The physical bridge port and completion timestamp are captured by the gateway;
neither is in the header. A destination, when defined, is in the payload.
The same source and body may appear on two ports; retain both observations.
A link ACK means a receiver asserted the ACK slot, not that a thermostat
applied a setting. CRC success and trailer success are separate results.

**Worked example.** A synthetic Rx record has type 01 and payload
`00 03 19 00 00 00 7d 71 23 42 05 82`: valid frame, port 3, timestamp 25 ms.
Header `7d712342` yields priority 1, ID `0x3d7`, subnet `0x12`, node 13,
and payload length 2. Payload `05 82` means error 5, instance 2, priority
2. The CRC over SOF and `7d7123420582` is `5f14`. That CRC is checked on the bus, not appended to the Rx payload.
The timestamp is gateway uptime, not wall-clock time.

## Payload notation

Offsets below start at the first byte **after** the header. A word (`u16`)
or double word (`u32`) is little-endian unless stated otherwise. Within a
byte, bit 0 is the least significant bit; `b[5:2]` includes both endpoints.
Lengths describe known fields, not a reason to discard extensions.

- **E** is an 8-bit temperature: Fahrenheit = `32 + 0.9 * raw`; `ff` is
  unavailable. **H** is a 16-bit temperature: Fahrenheit = `(raw - 850)/10`.
  The host preserves `4000`, `8000`, `c000` and `ffff` as unavailable H
  codes, and leaves zero raw rather than declaring an inactive field measured.
- **Setback codes:** 0 Wake, 1 Unoccupied (four-event), 2 Occupied
  (four-event), 3 Sleep, 4 Occupied (two-event), 5 Unoccupied (two-event),
  6 Away, 7 unknown. These are not the two-bit state fields in `0x2f7`.
- A **water subnet** byte encodes bus in bits 2:0, control in bits 7:3.
  This bus field is not the physical UART capture port.
- A short known layout yields `truncated`; unconsumed bytes in a known
  layout yield `extension_hex`. Unrecognized IDs retain the whole body.

## Decoded message IDs

| ID | Name | Payload layout |
| --- | --- | --- |
| `0x17e` | SnowmeltZoneStatus | Byte 0: source type [2:0], source ID [7:3]. Byte 1: count N. Then N ordered triples: key, low byte, high byte. Repeated keys are not collapsed. See key table below. |
| `0x1f2` | IOChangeReport | Subnet, node, channel, type (one byte each), then value u16. Channel type determines interpretation. |
| `0x22f` | SetpointStatus | Byte 0: enabled [0], setback [3:1], optimum start [5:4], priority [7:6]. Byte 1: actuator [1:0], demand [2], heat/cool [3], operating mode [5:4], thermostat priority [7:6]. Offsets 2/4/6: target, temp1, temp2 (H). Byte 8: auxiliary relay [1:0], requested enable [3:2], actual enable [5:4]. Bytes 9/10: status excuse codes. A twelfth byte seen on the bus is an extension. |
| `0x23f` | DHWStatus | Byte 0 as in SetpointStatus. Byte 1: actuator [1:0], demand [2], heat/cool [4:3], operating mode [7:5]. Bytes 2/3: target and tank (E). |
| `0x24f` | PlantOutputStatus | Byte 0: device count [3:0], device ID [7:4]. Byte 1: subtype. Remaining bytes use the subtype layouts below. |
| `0x2d7` | BlockRequest | Block ID u16, destination subnet, target nodes u32 bitmap. The 483 layout optionally appends arguments u32. An empty bitmap selects no node. |
| `0x2df` | BlockUpdate | Destination subnet, node byte (node [4:0], flags [7:5]), block ID u16, then block data. Blocks 5 (heat) and 6 (cool) carry all five E values in order: Wake, Unoccupied, Occupied, Sleep, Away. No support mask here. |
| `0x2e7` | BlockReport | Block ID u16, support mask u8, then block data. Bit 0 means logical byte 1. See the alignment rules below; do not treat every body as compact. |
| `0x31f` | RealTimeClockReport | Minute; weekday [2:0] and hour [7:3]; zero-based month [3:0] and DST [4]; zero-based day; year since 2000. Optional sixth byte: DST mode. Date byte ff means date unsupported. This is the sender's clock. |
| `0x32f` | OutdoorTemperatureReport | Outdoor H at offset 0. J1196J sends two copies (0x3dd8) and consumes the first (0x3e38). The host keeps the optional second word and reports whether they match. |
| `0x337` | MaximumBoilerPercent | One maximum-value byte. |
| `0x33f` | CycleLengthRequest | Air group ID, requested cycle length. |
| `0x367` | StageHeatDemand | Water subnet; stage [0], override [2:1], first [3], setpoint [4], DHW [5], priority [7:6]; pump delay [5:0], pump not required [6], output on [7]. Target H follows only if output on. |
| `0x36f` | AirGroupMemberStatus | Byte 0: group [4:0], cool vs heat [5], master-report request [6]. Byte 1: enabled [0], demand [1], schedule [4:2], optimum start [6:5], weather shutdown [7]. Offset 2: target E. Offset 3: local H. Byte 5: output 1 [1:0], output 2 [3:2]. Offset 6: remote H. |
| `0x3c7` | RelayStatusHeartbeat | One water subnet byte. |
| `0x3cf` | MasterStatus | Supply H, return H, then u16 with target [12:0] and heat/cool mode [15:13]. Bytes 6/7: loop and pump percentages. Byte 8: setback [2:0], weather shutdown [3], on max [4], on min [5], output limited [6]. Optional DHW supply H at offset 9. |
| `0x3d7` | NetworkError | Error code, then instance [5:0] and priority [7:6]. A native bus error is not a serial transport exception. |
| `0x3df` | NetworkInitRequest | No payload defined. |
| `0x3f7` | MasterQuery | No payload defined. |

### Block reports

Physical thermostat reports can retain the **positions** of unsupported
bytes through the last supported byte. For example, with mask `80` and
eight data bytes, the reported value is byte 8, not byte 1. This also
agrees with J1196J's fixed offsets in its block-report handler. An
unsupported byte is not a setting, even if its placeholder is nonzero.

The decoder uses positional offsets if the data reach the highest set
bit. It also accepts exact compact data (one byte per set bit, in
ascending order). Shorter than the supported count is truncated. A
length between that count and the positional span is ambiguous and
retained without assigning values. Extra bytes beyond a positional
layout are preserved. A zero mask conveys no supported value.

Block 0 identifies a product: bytes 1..3 form the little-endian product
number, bytes 4..5 the J number, byte 6 the Z number and byte 7 the
maximum tN4 version. Only complete, supported fields are interpreted.

### Weekly schedules

The 483's `ScheduleEvent` model defines a time byte in ten-minute units
from local midnight: 0 = 00:00, 143 = 23:50, 144 = no event. Other
values have no established time interpretation. There is no timezone
or absolute date. A missing day in a report is not a disabled event.

| Block | Fields in logical slot order |
| --- | --- |
| 000F | Heating schedule ID, cooling schedule ID, cycling rate, daily event configuration, optimum start operation, RTC setback enable. Cycling rates 0..3 correspond to 24-hour, 5-2, 5-1-1 and 7-day. Event configuration 1 = two events (Wake/Sleep); 2 = four. |
| 0010..0013 | One event per block: Wake, Unoccupied, Occupied, Sleep. Seven time bytes, Monday through Sunday. |
| 0038..003B | The same four events for indexed schedules. First byte: schedule ID [2:0], master flag [3], cycling rate [5:4], event configuration [7:6]. Then seven day bytes. |

In the 483 request queue the optional little-endian u32 BlockRequest
argument selects the secondary schedule ID, including ID zero. A
BlockUpdate carries destination subnet/node, block ID, and the new block
data, **not** a report support mask. Editing one event time requires
preserving the other days and, for indexed schedules, the whole selector
byte. Read the same block back to verify changes; a link ACK is not a
schedule confirmation. Single-schedule setup and all four weekday tables
have been observed in addressed reads; indexed layouts are reference-derived.

### Plant output subtypes

The subtype determines field widths, even for equipment with similar names.
Stage and pump masks below are packed low bits first. Temperature values
are raw; field width alone does not establish a temperature scale.

| Subtype | Fields after subtype |
| --- | --- |
| 0, 1 | Heat enable or cool enable, one byte. |
| 2, 3 | Modulating heat or cool: signal type u8, output level u16. |
| 4, 5 | Staged boiler or chiller: state [3:0] and pump [7:6]; four available stages [3:0] and four operating stages [7:4]; input and outlet u8. Only the boiler appends output [4:0] and return protection [7:5]. |
| 6 | Modulating boiler: same state and pump byte; current, minimum and maximum modulation; input and outlet u8; output and protection as for 4. |
| 7 | Modulating chiller: identified, but neither 483 layout defines the body. Returned unchanged as `extension_hex`. |
| 8..11 | Staged heat pumps (air-to-air, water-to-air, water-to-water, air-to-water): state [3:0], O/B [4], cooling [5]; stages on, stages available. Types 10/11 add a pump status byte [1:0]. Types 9..11 end with input and outlet u16. |
| 12..15 | Modulating versions of 8..11: same state byte; current, minimum and maximum modulation. Types 14/15 add pump status [1:0] and two temperature words. The 483 indexed and declarative layouts disagree by one byte on those words. A four-byte tail matches the packed layout; longer tails are returned as `temperature_tail_hex` without choosing offsets. |
| 16 | Staged electric: state, stages on, stages available, sensor u16. |
| 17, 18 | On/off injection pump or variable-speed pump: state, sensor u16. |

### Snowmelt status keys

Each value occupies two bytes even when it carries a small enum or a flag.
The decoder does not convert a raw word to a percentage or temperature
unless its encoding is known.

| Hex key | Value |
| --- | --- |
| 01..18 | Command status; retain the individual key |
| 19, 1a | Water detection, heat sequence |
| 1b | Operating substate in low byte, state in high byte |
| 1c, 1d, 1e, 1f | Warm-weather shutdown, cold-weather cutoff, zone type, electric off-time (15-second ticks) |
| 20, 21 | Zone ID, sensor flags |
| 22, 23, 24 | Slab temperature, slab setpoint, edge temperature (H) |
| 25, 26 | Melt and storm group IDs |
| 27, 28, 29 | Boiler, mixing and PWM outputs, raw |
| 2a, 2b | System pump and alert-relay contacts |
| 2c, 2d, 2e | Return, supply and supply target, raw |
| 2f, 30 | Keep-alive, melting time (minutes) |
| Others | Preserve key and raw u16 |

## Other IDs in the 483 registry

These are additional decoded layouts, not a claim that every model
supports them. Fields are listed in transmitted order.

| ID | Name | Fields or interpretation in the reference |
| --- | --- | --- |
| `0x010` | ZoneSynchAdjustment | Synchronization, purging and target mode, one byte each. |
| `0x011` | ZoneSynchQuery | No payload defined. |
| `0x173` | TankStatus | Tank ID, pump output percentage, tank reading, target, each one byte. The reference's tank validity test compares a byte with a word sentinel; do not infer a scale from it. |
| `0x17f` | SnowmeltZoneStatusRequest | Destination type [2:0], ID [7:3]; count; that many one-byte keys. Unlike the report, no values follow the keys. |
| `0x1f3` | IORequestResponse | Same subnet/node/channel/type/u16 value layout as IOChangeReport. |
| `0x1f4` | IORequest | Subnet, node, channel. |
| `0x1f5` | IODiscoverReport | Subnet, node, channel, channel count, six label bytes, type, u16 value. |
| `0x1f6` | IODiscoverRequest | Subnet, node. |
| `0x200` | SecureGatewayAccess | Access-status/control message, not thermostat telemetry. |
| `0x20f` | SetpointEnableReport | Group ID, status byte: requested [0], actual [4]. Neither establishes slab temperature. |
| `0x21f` | SetpointRemoteEnable | Group ID and enable byte. |
| `0x25f` | FanDuctStatus | Fan 1 [1:0], fan 2 [3:2], optimum start [5:4], freeze protect [6]; duct and coil u16 readings. |
| `0x26f` | SlabStatus | Slab status [2:0], target byte, current u16. |
| `0x287` | CalibrationMode | One mode byte. |
| `0x2ef` | NetworkLock | Access/address lock flags. |
| `0x2f7` | ScheduleStatus | Byte 0: schedule ID [1:0], master [2], previous ID [4:3], previous master [5], event configuration [7:6]. Byte 1: current/next states [1:0]/[3:2], duplicate response [4]. Byte 2: time to next edge. This is status, not a weekly transition table. |
| `0x2ff` | SceneReport | Current scene, duration u16. |
| `0x307` | WWSDRequest | No payload defined. |
| `0x30f` | WWSDStatus | Warm-weather shutdown flags [0..4]: Wake, Unoccupied, Occupied, Sleep, Away. |
| `0x317` | RealTimeClockRequest | No payload defined. |
| `0x322` | RHQuery | Subnet and four-byte node selector. |
| `0x323` | RHReport | Count N, flags, control value (J1196J caches byte 2 as relative humidity), dewpoint u16, internal sensor u8. Preserve the next N bytes as a group: the 483 indexed reader overlaps its final sensor and first target. Both 483 layouts place optional humidity/dehumidity targets at offsets 6+N and 7+N. An incomplete declared sensor region is truncated, not zero-filled. |
| `0x327` | OutdoorTemperatureRequest | No payload defined. |
| `0x347` | MixingModulePresent | Actuator type [3:0], adjustable speed [6], controllable system pump [7]; motor speed byte. |
| `0x34f` | MixingModuleReport | Mode/logic/demand/latch/limit/fault flags; pump output; actuator percentage and direction flags. Byte 3 packs the high two bits of target, supply and return in [1:0], [3:2], [5:4] plus return/supply sensor fault bits. Bytes 4..6 carry the respective low bytes. |
| `0x357` | MixingModuleUpdate | Output-mode/control flags and target high bits; target low byte; target pump output. An optional motor speed is declared, but the reference does not resolve its latch predicate. |
| `0x35f` | StageHeatDemandRequest | Target subnet, output level [6:0] and stage [7], target node bitmap (three bytes little-endian, unlike a BlockRequest). |
| `0x363` | GeneralDemand | Water subnet; cooling [6] and demand [7]. |
| `0x36a` | AirGroupMasterConfig | Group [4:0], no-response [6], group master [7]. Second byte: previous group [4:0] and previous master [7]. |
| `0x377` | AirGroupMasterStatus | Group and inhibit flags, cycle length/start, heat and cool status/targets/readings, output states, remote reading. Not interchangeable with a member report. |
| `0x37f` | WaterCycleStart | Cycle length [6:0], hold override [7]. |
| `0x387` | NetworkTestTime | Additional time u16. |
| `0x38f` | NetworkTestMode | Test mode byte and expected duration u16. |
| `0x397` | EndPurge | Target subnet, output level [6:0] and stage [7], target node bitmap. |
| `0x39f` | StartPurge | Same target layout as EndPurge. |
| `0x3a7` | StartOverrideOn | Same target layout as EndPurge. |
| `0x3af` | EndOverrideOff | Same target layout as EndPurge. |
| `0x3b7` | EndOverrideOn | Same target layout as EndPurge. |
| `0x3bf` | StartOverrideOff | Same target layout as EndPurge. |
| `0x3ce` | PlantLoopStatus | Supply, return, packed mode/target, loop/pump percentages and status flags as in the nine-byte base MasterStatus layout. |
| `0x3e7` | TakingAddress | Previous subnet and node; the frame header identifies the sender. |
| `0x3ef` | AddressTaken | No payload defined. |
| `0x3ff` | MasterPresent | Boiler subnet followed by zero to seven mixing subnets, according to payload length. |

For regression vectors use [tests/payload_test.py](../tests/payload_test.py).
For electrical activity, CRC failures, trailer faults and dropped UART
records, see [receiver diagnostics](interfaces.md#receiver-diagnostics).
