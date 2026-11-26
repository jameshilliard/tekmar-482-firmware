# Native tN4 frame reference

Use this reference to interpret bytes from a 482 running the custom bridge.
The [UART interface](interfaces.md#stream-decoded-frames) transports these
frames. The stock 482's TPCK/tRPC methods are a different protocol: for
example, native `0x31f` is a remote clock report, not a request to read the
482's clock.

The catalog below covers the 62 message IDs registered in the 483 SW 11
`message_types.rb` and `message_types_in.rb` registries. The [host
decoder](../tools/payloads/__init__.py) recognizes all 62. A registration is evidence
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
Six consecutive dominant bits can also be an **error flag**, not a new
frame. A Normal-mode receiver or transmitter can signal a fault this way;
the delimiter ends after eight consecutive recessive bits. Raw mode does
not add error flags to the bus. See [link recovery](normal-mode.md#bus-compatibility)
for the fault and arbitration rules.

**Worked example.** A synthetic Rx record has type 01 and payload
`00 03 19 00 00 00 7d 71 23 42 05 82`: valid frame, port 3, timestamp 25 ms.
Header `7d712342` yields priority 1, ID `0x3d7`, subnet `0x12`, node 13,
and payload length 2. Payload `05 82` means error 5, instance 2, priority
2. The CRC over SOF and `7d7123420582` is `5f14`. That CRC is checked on the bus, not appended to the Rx payload.
The timestamp is gateway uptime, not wall-clock time.

For the commands the 483 application actually originates, including group
selection, clock daylight handling and their limits, see
[gateway-originated traffic](gateway-traffic.md).

## Payload notation

For the distinction between an air-group operating target, the configured
heat/cool table, and an inhibited or inactive channel, see
[live room setpoints](normal-mode.md#bus-compatibility). Raw capture
retains both channel bytes regardless of eligibility.

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
| `0x22f` | SetpointStatus | Byte 0: enabled [0], setback [3:1], optimum start [5:4], priority [7:6]. Byte 1: actuator [1:0], demand [2], heat/cool [3], operating mode [5:4], thermostat priority [7:6]. Offsets 2/4/6: little-endian target, temp1 and temp2 words. No units or application selector in the frame. Byte 8: auxiliary relay [1:0], requested enable [3:2], actual enable [5:4], reserved [7:6]. Bytes 9/10: status excuse codes. Preserve extensions. |
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

Thermostat mode is the supported first byte of block `0003`, not a
heat/cool bit in an air-group report. In 483 SW 11 the native codes are
0 Off, 1 Heat, 2 Auto, 3 Cool, 4 Vent, 5 Humidity, 6 Emergency and
7 Backup. This is a native vocabulary, not an extension of stock tHA's
ModeSetting command. The 483 names more modes than it offers in its remote
selector. It checks cooling stages (block `0004`) and enabled heat pumps
(`0097`) for Auto/Cool, offers Vent for 530/540, Emergency for 557 or a
heat-pump-enabled 553, and omits Off for 527/537. A 538 at revision G or
later has no remote mode adjustment. Do not offer an installation-dependent
choice just because its block appears in a model's catalog.

A `36f` member reports one operating channel; a `377` master reports
both independently. In each channel the enable bit and the demand bit
are distinct. Keep both, including simultaneous heat/cool indications;
neither confirms the user's mode selection. A changed operating channel
can prompt an addressed read of block `0003`. A `2df` setting update is
also only a request: the device's subsequent `2e7` report is the evidence.

**Thermostat targets and outputs.** Blocks `0005/0006/0007/0009/A4/A5`
have separate Wake, Unoccupied, Occupied, Sleep and Away bytes. Block 7
adds a floor maximum, not another setback. Only supported positions are
settings. Heat, cool and floor use degE. Block 9 is a ventilation
requirement, not fan speed: zero means Auto. The J1196J block handler
copies its bytes unchanged to FanPercent; [tekmar's FanPercent specification](https://www.watts.com/our-story/brands/tekmar/tha)
uses steps of 10% for models 544/545/546 and 1% otherwise. Block 8's fan
setup is separate: SW 11 accepts a stage assigned fan 1 or 2, or enabled
ventilation, except that ventilation on both fans is off for a two-speed
fan. An absent setup byte is not a disabled fan.

`25F` carries two two-bit output states and duct/coil words, not a percent.
In `26F`, J1196J at `0x6214` uses the full status octet: only 0 and 1
carry a primary floor target. Other values are alternate targets. The
measured floor word is independent. In `323`, the control RH, optional
current humidify/dehumidify targets and raw sensor bytes are distinct.
Keep the dewpoint word raw unless its unit is independently established.
`A3` enables each actuator separately; `A4/A5` are its **stored**
setback tables. Neither a humidity measurement nor the current targets
establish that an actuator is configured or that a stored slot changed.

The [addressed block catalog](tn4-blocks.md) covers the 166 block IDs in
the 483 SW 11 registry, including plant, pump and heat-pump diagnostics.

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

| Block | Positional bytes (in wire order) |
| --- | --- |
| 0003 | Operating mode, heating cycles per hour, air-group ID. |
| 0005, 0006 | Heat or cool targets: Wake, Unoccupied, Occupied, Sleep, Away (degE). |
| 0007 | The same five floor targets, followed by the floor maximum (degE). |
| 0009 | Five ventilation targets in the same order; the wire scale depends on the thermostat model. |
| 000D | Onboard sensor configuration, then external sensor configurations 1 through 4. Zero is off; 2 is floor. |
| 0026 | Setpoint application mode, mode configuration, run mode configuration, relay 2 sampling/start/duration, setpoint-enable address, outdoor temperature enable. |
| 0028, 002A, 002C, 002E, 0030 | Setpoint, hot tub, DHW, floor and pool targets respectively. Four little-endian degH words: wake/occupied, unoccupied/sleep, away, maximum. Some floor controls omit the last word. Both bytes of a word must be supported to decode it. |
| 0029, 002B, 002D, 002F, 0031 | Secondary tables for the same applications: wake and sleep as two little-endian degH words. These are not the tables used by tHA SetpointDevice. |
| 00A3 | Humidification mode setup, dehumidification mode setup. |
| 00A4, 00A5 | Five humidification or dehumidification percentage targets in setback order. tHA exposes only the first; the native blocks contain all five. |

These layouts come from the 483 block types and the J1196J block-report
and block-update handlers. Reports include a support mask; updates do not.

### Addressed snowmelt blocks

These are native block IDs for `2D7` requests, `2E7` reports and `2DF`
updates, **not** keys for a `17F` zone query or group IDs for a `21F`
enable command. Identify the device from block 0: the 483 catalog maps
tektra product 105801 / J1226 to model 654. Its model-specific support
table lists all blocks below **except 00CD**, which is in the general
registry. Use each device's reported mask, not the catalog, as evidence
of a value. Addressed reads from a physical 654 returned **all 32** of
these blocks, including 00CD: selector zero returned a three-byte report
with support mask 07, relay ID and a little-endian cycle count. The model
table is not an upper bound on device capabilities. The test used
selector zero on indexed blocks and did not change settings.

| Block (hex) | Data bytes, in order |
| --- | --- |
| 0071, 0072, 0078 | Melt, idle, storm slab targets respectively: one degH u16. Idle and Storm encode Off as zero; Melt has no Off setting. |
| 0073 | Water sensitivity u8. |
| 0074, 0075, 0079 | Manual melt run time, additional melt time, storm run time, respectively: u16 minutes. |
| 0076, 0077 | Cold-weather cutoff and warm-weather shutdown target, respectively: raw u16. |
| 007A, 007B | Sensor ID u8 and configuration u8: snow/ice sensor type or slab sensor selection. |
| 007C, 007D | Slab protection u8; maximum melt time u16 in **hours**. |
| 00C0..00C4 | One u8 per block: zone ID, sensor ID, melt group, storm group, tracked zone. |
| 00C5 | Sensor ID u8, melt start type u8. |
| 00C6..00CB | Sensor ID u8 and raw u16: slab high/low, supply high/low, return high/low, respectively. |
| 00CC, 00CD | Relay ID u8 and u16 hours or cycles. |
| 00CE, 00CF | Sensor ID u8 and u16 wet or dry level. |
| 00D0 | Application mode u8, application type u8. The 483 names mode values 0..4 PWM, boiler, mixing, electric and sensor-only. |
| 00D1, 00D2 | Control ID u8 and raw u16 boiler minimum or maximum. |

The 483 sends the eleven-byte request layout with a little-endian u32
argument, zero for unindexed blocks. For 007A/B, 00C5..CF and 00D1/D2,
it selects the sensor, relay or control whose one-byte ID appears first in
the report. ID zero is valid. 00C1 reports the configured sensor ID; it
does not accept a sensor selector. The two bytes of a u16 must both be
supported to interpret it; a single supported byte is kept raw.

The 483's snowmelt adjustment editor writes the two-byte target for
Melt, Idle or Storm. Its UI uses 20 F as a display substitute for Off,
but its encoder sends **0000**, not the degH encoding of 20 F (1050).
Changing a target does not start or stop the system. Read the block
back for confirmation. The other layouts describe data, not evidence
that a device accepts writes to counters, sensors or installer settings.
See the [654 manual](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/81803-source/638820695340000000/654-d-03.pdf)
for operating conditions and the distinction between set temperatures
and enabling a group.

### System scenes

The 483 SW 11 `MessageQueue#push_scene_change` sends a priority-zero
`0x2FF SceneReport` on all ports. The address in the native header is the
sender, not a target thermostat. The three data bytes are the zero-based
scene ID and a little-endian duration in minutes. For example, at source
0/0 the entire native body for scene 8 is `2f f0 00 03 07 e0 01`.
A Raw host chooses physical ports explicitly and gets a separate link
result for each. There is no per-device scene confirmation in this exchange.

| Display | Wire ID | 483 duration (minutes) | Participating device's response |
| --- | --- | ---: | --- |
| 1 Normal | 0 | 0 | Configurable |
| 2 Away | 1 | 0 | Away |
| 3 Unoccupied | 2 | 0 | Unoccupied |
| 4 | 3 | 0 | Configurable |
| 5 | 4 | 0 | Configurable |
| 6 Temporary Occupied | 5 | 180 | Occupied |
| 7 | 6 | 240 | Configurable |
| 8 | 7 | 480 | Configurable |

The [User Switch 480 manual](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/81552-source/480-u-06.pdf)
describes device participation and the return to the previous permanent
scene on expiry. Listen for `2FF` and retain its source, port and actual
duration. A local transmit echo or link ACK is not a report from a
thermostat. No current-scene query is identified in the 483 registry.
A listener joining mid-scene cannot reconstruct it from a device's
[scene setup](tn4-blocks.md#scene-response-settings) or its current setback.

### Weekly schedules

The 483's `ScheduleEvent` model defines a time byte in ten-minute units
from local midnight: 0 = 00:00, 143 = 23:50, 144 = no event. Other
values have no established time interpretation. There is no timezone
or absolute date. A missing day in a report is not a disabled event.

| Block | Fields in logical slot order |
| --- | --- |
| 000F | Heating schedule ID, cooling schedule ID, cycling rate, daily event configuration, optimum start operation, RTC setback enable. Cycling rates 0..3 correspond to 24-hour, 5-2, 5-1-1 and 7-day. Event configuration 1 = two events (Wake/Sleep); 2 = four. |
| 0010..0013 | One event per block: Wake, Unoccupied, Occupied, Sleep. Seven time bytes; normally Monday through Sunday, with the revision exceptions below. |
| 0038..003B | The same four events for indexed schedules. First byte: schedule ID [2:0], master flag [3], cycling rate [5:4], event configuration [7:6]. Then seven day bytes. |

In the 483 request queue the optional little-endian u32 BlockRequest
argument selects the secondary schedule ID, including ID zero. A
BlockUpdate carries destination subnet/node, block ID, and the new block
data, **not** a report support mask. Editing one event time requires
preserving the other days and, for indexed schedules, the whole selector
byte. Read the same block back to verify changes; a link ACK is not a
schedule confirmation. Single-schedule setup and all four weekday tables
have been observed in addressed reads; indexed layouts are reference-derived.

**Owner and calendar.** In 483 SW 11 `Schedule::SCHEDULE_TYPES`, a setup
ID of 0 means no schedule, 1 a device's zone schedule, 2..5 master of
system schedule 1..4, 6..9 member of system schedule 1..4, and 10
Unoccupied. A member is not the owner of a second editable copy. On a
033 timer, query selectors 0..3 for system schedules 1..4; the master
bit in the indexed Wake block identifies slots the timer actually owns.
For thermostats with only a heating schedule ID the 483 also uses it for
cooling. Its per-model tables specify an independent cooling ID for 540,
545 and 546; a missing cooling byte on one of these is unknown, not heat.

**Live role changes.** `2F7 ScheduleStatus` identifies a change when the
previous and current two-bit IDs differ or the master bit changes. The
reported ID is zero-based, not a block 000F binding. In particular, losing
mastership does not tell the host whether the device selected a member
schedule, its zone schedule, or None. SW 11 re-reads 000F on an identified
non-timer and requests the event rows when mastership is acquired. Use
confirmed setup to decide which rows to display. For an 033, use the
indexed Wake header (0038) for each affected system instead of 000F.
Different timer slots can be reported by the same physical source.

For two-event status, state 0 or 2 means Occupied; 1 or 3 means
Unoccupied. Four-event status uses Wake, Unoccupied, Occupied, Sleep in
order. Neither zero events nor reserved event code 3 names an active
period. The reference uses the current event from a **master** to update
its schedule status. Ordinary event changes, duplicate-response bit and
time-to-next-edge changes are not evidence that weekly times changed.
Retain the edge byte without assuming a unit or calculating a wall time.

Daily event codes 0/1/2 mean zero/two/four timed events. In a two-event
schedule the **Wake** and **Sleep** wire rows are displayed as Occupied
and Unoccupied, not the separate Occ/UnOcc four-event rows. A 24-hour
edit applies the same time to all seven days; 5-2 groups Monday-Friday
and Saturday-Sunday; 5-1-1 separates the weekend days; 7-day changes one
day. Preserve the reported bytes on reads even if grouped days disagree.

**Revision exceptions.** Block zero's Z byte is zero-based (0 = A). The
483's `BlockFixups` puts Sunday first on the wire for 552 A-N and 557 A-C
in single-schedule blocks 10..13 only. Translate both the weekday and its
support bit when reading, editing, and verifying readback. Without the
affected device's revision, retain wire slots but do not assign weekdays.
The 483 also documents 545/546 through K losing mastership on a setup
rewrite. Do not rewrite 0F when it could demote an existing or proposed
heat or cool master; editing an event row does not rewrite 0F.

A 553 can return exactly five positional setup bytes with no sixth support
bit. The 483 writer's sixth byte is a literal zero, so this layout is
sufficient to construct its six-byte write. Do not invent any of the first
five bytes from a shorter report or clear a nonzero sixth byte.

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

**Equipment identity.** The 483 SW 11 plant logger indexes staged boilers and
heat pumps by subtype and device ID. Retain that pair *within* the reporting
port, header subnet and node. Neither the device ID nor the low count nibble
is a tN4 address, loop number or an addressed runtime register selector. A
modulating boiler can report ID zero and total zero. Keep the report; do not
infer that a zero or reduced total removes other equipment. An observed
output list is not a confirmed inventory.

For a staged boiler or chiller the two masks are independent and stage 1 is
bit zero. A heat pump or staged electric output reports **counts**, not
numbered stages. A ratio of stages on can only be derived when the available
count is nonzero and the on count does not exceed it. It is not a modulation
percentage. Neither a pump's two-bit status nor a six-bit mixing actuator
field is a measured boiler firing percentage.

**Demand and commands.** StageHeatDemand identifies a water subnet in the
payload and one of two stages. Keep both alongside the source; do not use
the header subnet as the target loop. When output-on clears the target word
is absent, not the previous target. GeneralDemand and RelayStatusHeartbeat
also identify a water subnet. A heartbeat is not a relay position.
MaximumBoilerPercent is a limit, not a measured output. WaterCycleStart
announces a cycle, not a valve or pump operation.

MixingModulePresent declares capabilities and motor speed. MixingModuleReport
is the measured status, including two separate sensor fault flags and three
packed ten-bit words. MixingModuleUpdate is a command; its target and pump
level must not replace those reported values. Neither 483 layout establishes
the predicate for interpreting a fourth command byte as motor speed. Preserve
the whole tail. There is no matching host query in the 483 for an immediate
plant output or mixing status snapshot; MasterQuery requests topology.

### Snowmelt status keys

The 483 SW 11 `snowmelt_request_for` uses application block D0 to choose
an address. In control modes 0..3 it reads the configured **zone number**
from C0 (1..12); in sensor-only mode 4 it reads the **sensor number** from
C1 (1..24). The 17F destination byte is `((number - 1) << 3) | role`,
with role 1 for a zone and 2 for a sensor. The 17E report source byte is
`(number << 3) | role`. Neither number is the native node in the header
or a melt/storm enable group. On a physical 654, a read using the SW 11
gateway priority (0) returned all three requested telemetry keys from the
expected native source, **not in request order**. Match actual keys in
complete triples; never count an extension byte as a fourth key.

The 483 requests water detection and slab temperature for every application.
For a controller it also requests operating state and slab target, then
application-specific outputs: PWM/boiler/mixing use supply temperature
and system pump; boiler/mixing also use supply target. Electric uses PWM
output. The [query catalog](../tools/payloads/queries.py) supplies those
sets. They are useful requests, not a promise that every key is reported.
The six-bit frame length fits at most 61 request keys or 20 report pairs.
Multiple reports can contribute; a missing key is unknown, not zero.

A single native source can emit **both** a zone report and a sensor
report, even using the same configured number. SW 11 uses the zone's slab
and water measurements for a control application and the sensor's for a
sensor-only application. Keep each `(physical port, subnet, node, report
role, report number)` separate. Match the currently configured endpoint
before presenting a primary measurement. Within a report the triples are
ordered, including duplicate keys; the last occurrence updates the value.
An unknown role/number or a short final triple must not update device
state. Trailing bytes after the declared count are extensions, not keys.

For remote start/stop the [654 manual, Melt and Storm Groups](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/81803-source/638820695340000000/654-d-03.pdf)
specifies **group enable**, not a write to status key 1b. In control
applications read C2 for the melt group and C3 for the storm group, then
send `21F [group, enable]` on the selected line. It is a group broadcast;
another device with the same assignment can respond. Do not send this to
sensor-only applications or use a zone number as the group. After the
link result, `17F` for key 1b at the original zone reads its operating
state. A `20F` group report, echo, or link ACK is not a zone-state report.

Each value occupies two bytes even when it carries a small enum or a flag.
The decoder does not convert a raw word to a percentage or temperature
unless its encoding is known.

| Hex key | Value |
| --- | --- |
| 01..18 | Command status; retain the individual key |
| 19, 1a | Water detection, heat sequence. For 19 the 483 names 1 absent, 2 dry, 3 wet, 4 open, 5 short and 6 overheated. Preserve the raw word for other states; an electrical fault is not a dry sensor. |
| 1b | Operating substate in low byte, state in high byte (1 off, 2 idle, 3 storm, 4 melting) |
| 1c, 1d, 1e, 1f | Warm-weather shutdown, cold-weather cutoff, zone type, electric off-time (15-second ticks) |
| 20, 21 | Zone ID, sensor flags |
| 22, 23, 24 | Slab temperature, slab setpoint, edge temperature (H) |
| 25, 26 | Melt and storm group IDs |
| 27, 28, 29 | Boiler, mixing and PWM outputs, raw |
| 2a, 2b | System pump and alert-relay contacts. For 2a the 483 recognizes 0 off, 1 on. Do not infer pump state from output level or group demand. |
| 2c, 2d, 2e | Return, supply and supply target temperatures (H); the 483 maps these keys to its degH temperature model. |
| 2f, 30 | Keep-alive, melting time (minutes) |
| Others | Preserve key and raw u16 |

### Other read-only queries

The same catalog pairs zone synchronization 011/010, clock 317/31F,
warm-weather shutdown 307/30F, outdoor temperature 327/32F and master
presence 3F7/3FF. Each request has an empty payload. MasterPresent lists
subnets, not water temperatures. The clock belongs to the reporting peer,
not the 482. Neither a positive link result nor silence establishes the
presence, value or absence of the requested service.

I/O discovery 1F6/1F5 and channel reads 1F4/1F3 use the full payload
subnet and node bytes, distinct from the six-bit native header node.
Discovery also carries a channel count; count distinct channel IDs per
reporting source, without assuming numbering starts at zero. For a
channel read match the payload channel too. An unsolicited 1F2 change
report remains telemetry, not the 1F3 service. A 1F5 discovery reply
contains a current sample as well as metadata; a 1F3 or 1F2 omits the
count and label rather than clearing them. Keep all six label bytes,
including embedded zero and non-ASCII bytes, and any trailing extension.
A positive consistent count must be met by distinct IDs from the same
sender and payload endpoint in one discovery exchange. A partial reply,
a count change, and a link ACK do not prove a channel disappeared.

The 483 SW 11 `IoChannel` model names these channel types (the value is
always a little-endian u16):

| Type | Interpretation |
| --- | --- |
| 0, 255 | Unused, disabled. Neither means an active output of zero. |
| 1, 7 | Relay, demand: 0 off/inactive, 1 on/active. Keep other words raw. |
| 2, 3, 4 | 0-10 V output, 0-10 V input, 4-20 mA output. The raw-to-engineering scale is not established by the wire layout. |
| 5, 10 | Sensor(H), Sensor(F). The I/O model does not establish a usable temperature conversion for these samples. |
| 6, 9, 11 | Humidity, variable-speed pump, PWM cycle. Values 0..100 can be displayed as percent; preserve other words without assigning a percentage. |
| 8, other | Integer, or unknown type: preserve the type ID and value. |

The 483 terminal-reference tables cover models 553 (channels 0..5), 557
(0..9), and 654 (0..10). These are board labels, not discovery results or
proof a relay is active. For example, 654 channels 1, 2 and 3 are all
marked `Mod`; they remain three distinct IDs. The wire label is separate.
These services provide reads and observations, not a generic I/O write.

Humidity query 322 carries
a subnet and four-byte little-endian selector. The 483 calls it
`node_id`; a BlockRequest bitmap conversion is not established here.
Keep the selector raw and match 323 by its reporting source.

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
| `0x2ff` | SceneReport | [System scene announcement](#system-scenes), not addressed device state. |
| `0x307` | WWSDRequest | No payload defined. |
| `0x30f` | WWSDStatus | Warm-weather shutdown flags [0..4]: Wake, Unoccupied, Occupied, Sleep, Away. |
| `0x317` | RealTimeClockRequest | No payload defined. |
| `0x322` | RHQuery | Subnet and four-byte node selector. |
| `0x323` | RHReport | Count N, flags, control value (J1196J caches byte 2 as relative humidity), dewpoint u16, internal sensor u8. Preserve the next N bytes as a group: the 483 indexed reader overlaps its final sensor and first target. Both 483 layouts place optional humidity/dehumidity targets at offsets 6+N and 7+N. An incomplete declared sensor region is truncated, not zero-filled. |
| `0x327` | OutdoorTemperatureRequest | No payload defined. [Normal mode](normal-mode.md) answers with the selected outdoor word in a 32F broadcast on all five lines. |
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

**Associating water temperatures.** A MasterPresent slot is a subnet byte
(control in bits 7:3, bus in 2:0), not a physical gateway port. Slot 0
advertises the boiler; slots 1 through 7 advertise those numbered mixes.
A zero byte is unassigned. SW 11 compares the bus bits of the sender and
slot before associating the arrival port. A master can advertise remote
subnets, and the same MasterStatus can be heard on multiple ports. Keep the
physical port, full header subnet and sender node. Prefer an exact subnet
match; a bus-only match must be unique. Never assign the status to every
slot or count an unlabelled copy as a second boiler.

The 483 prefers explicit BusLoopSetup blocks over positional names.
Block `0033` describes slots 1-4, `0034` slots 5-7. Each slot is a pair
of independently supported bytes: subnet and heat-source ID. Only interpret
a pair present in the same report. A zero support mask supplies no map; a
supported subnet with no corresponding ID is an unresolved assignment,
not permission to use the positional name. In the ID, 7:6 are operating-mode bits,
5:4 are the type (0 mixing, 1 tank, 2 unknown, 3 boiler), and 3:0 are the
loop number; a completely zero ID explicitly unlinks the loop. Use the
on-wire mixing number, not that number plus one. A setup slot number is not
a mixing number. Do not equate the low nibble with the ID in TankStatus.

MasterQuery solicits the topology, not a fresh measurement. Keep the
independent MasterStatus and PlantLoopStatus samples separate. Only a
MasterStatus of at least eleven bytes carries a complete DHW supply word;
a tenth byte alone is an opaque tail, not an invalid base status.
DHWStatus (`023f`) carries degE temperatures and demand flags, whose
application depends on its sender (setpoint control or reset control).
TankStatus (`0173`) uses separate raw temperature bytes; keep each tank
ID and do not borrow the DHWStatus scale.

For regression vectors use [tests/payload_test.py](../tests/payload_test.py).
For electrical activity, CRC failures, trailer faults and dropped UART
records, see [receiver diagnostics](interfaces.md#receiver-diagnostics).
