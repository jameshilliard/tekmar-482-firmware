# Native addressed blocks

These are inner block IDs, not tRPC methods or outer tN4 message IDs.
Use a native `2D7` request with the reporting node's address on its
physical line. A `2E7` report carries a support mask; a `2DF` update
does not. The [frame reference](tn4-frames.md#block-reports) describes
compact and positional reports. Unsupported placeholders, including
nonzero ones, do not establish a reading or a writable setting.

The table covers the 166 IDs in the 483 SW 11 block registry. The
[field layouts](../tools/payloads/layouts.py) give names and widths in
wire order; words and double words are little-endian. For indexed blocks
the request argument identifies the selected instance. Its report
includes the selector in byte 1, except that a schedule ID shares that
byte with flags. A partial word remains raw. Unknown units, reserved flag
bits and extension bytes are retained rather than assigned meanings.
Layout support does not mean every product answers or accepts an update.

To compare against trusted, locally extracted 483 software, run
`python3 tests/compare_483.py <sw07-tn4-dir> <sw11-tn4-dir>` (the directories
contain `message_types.rb`; Ruby 3 is needed). This is offline and uses no
equipment. It checks message registrations, block IDs and indexed selectors,
exercises every bit of the SW 11 indexed block readers and compares actual
writer bytes with field edits. Matching bit positions alone does not
establish the engineering unit or a particular device's capabilities.

Two SW 11 indexed reader defects should **not** be copied: for block 08
it reads the whole first or second byte for two fan fields declared as
two bits, and for block 2B it ANDs instead of ORing the two halves of each
hot-tub target. Use the low two bits for fan type and cool stage 2, and two
little-endian words for the hot-tub wake/sleep table. The independent
declarative layouts and the 2B write setters agree with these interpretations.

### Discovering identities

Block 00 can select multiple nodes with a single read. The four-byte
little-endian target mask has bits 0..30 for nodes 1..31, irrespective
of the node lock bit. Bit 31 is padding. Use `ff ff ff 7f` and argument
zero to ask for identity on one subnet. Each answering node sends its
own ordinary block report. This also reached thermostat nodes that had
not sent status during a short observation and a snowmelt control.
Reports may be forwarded onto other physical lines. A receive port is
not proof of the original wiring or of a unique device.

The [product catalog](../tools/payloads/products.py) uses Tektra and J
numbers from the 483 SW 11 product table, plus the additional models
in the [published tHA list](https://www.watts.com/our-story/brands/tekmar/tha).
It does not infer a model from a J number alone, a group ID or a master role.
Two early thermostat variants share a product number; so do a power
manager and a user switch. Without a distinguishing J number they
remain unidentified. For identified models, the 40 SW 11 read tables
suggest blocks to try; the masks in actual reports establish the
available data, not these lists.

SW 11's `Block.fixup_device_identification` corrects four early devices
that transmit a byte-swapped J number. With a matching cataloged product,
the 541 reports 24068 instead of 1118, the 542 reports 23556 instead of
1116, the 161 reports 29956 instead of 1141, and the 162 reports 30212
instead of 1142. The 537 has a different exception: a **reported** product
of zero together with J1161 means product 100102. A missing product field
is not zero. A supported J of zero makes the entire identification report
unusable; a missing J can still leave an unambiguous product number.
Raw capture and the decoded `product_number` and `j_number` retain the
reported values. The decoder adds a `corrected_product_number` or
`corrected_j_number` only where the corresponding quirk applies.

| Block (hex) | Report contents |
| --- | --- |
| 00 | Product (24 bits), J number (16), Z number and maximum tN4 version. Product is not the retail model number. |
| 01..04 | Stage terminals and pump flags; reset curve and supply limits; heating setup; cooling stages/times/interlock. Each stage's pump byte has delay in the low 7 bits and required in bit 7. |
| 05..09 | Heat, cool and floor setback targets; fan configuration; ventilation targets. Fan configuration uses four 2-bit fields in byte 1 and two more in the low nibble of byte 2. |
| 0A..0F | Filter replacement interval and elapsed time (two words), warm/cold weather shutdown targets, sensor assignments, scenes, schedule setup. |
| 10..14 | Single-week event tables; preferences (access, units, backlight, time format, DST, exercise). |
| 15..1A | Room average/minimum/maximum; onboard sensor and sensors 1..4 each have a configuration byte and three words for current/minimum/maximum. A sensor value is not necessarily a temperature. |
| 1B..21 | Heat stage 1/2, cool stage 1/2 and fan 1/2 outputs; the next six blocks hold one accumulated-time word each in the same order. |
| 22..27 | Mixing output/actuation, boiler modulation/cycling, DHW setup, setpoint setup, application setup and setpoint-control operating parameters. DHW and setpoint exchange targets follow wake, unoccupied, occupied, sleep, away. |
| 28..32 | Five pairs of application target tables, followed by manual override time. See [application layouts](tn4-frames.md#block-reports). |
| 33, 34 | Subnet/heat-source byte pairs for buses 1..4 and 5..7. Subnet low 3 bits select the water bus; high 5 bits identify the control. |
| 35..37 | Available/enabled boiler masks, three DHW tank setback targets and differential, local permanent override state. |
| 38..3B | Multi-schedule weekly event tables. Selector is the low 3 bits of byte 1. See [schedules](tn4-frames.md#weekly-schedules). |
| 3C, 3D | Selected boiler: enable/type/relay and pump purge times; thermal mass, actuator speed, ignition and modulation limits. |
| 3E..42 | External signal and EMS setup, plant feature and sequencing options, primary/DHW pump setup, demand proofing delays. |
| 43, 44 | Boiler or pump selector (u8) followed by runtime hours (u16). |
| 45 | Boiler number (u8), minimum and maximum output (u16 each). |
| 46, 47 | Heating or cooling source ID, three u16 PID coefficients (P, I, D), inhibit time (u8). |
| 48, 49 | Per-port link counters. 48: stuffing, CRC, CRC delimiter, ACK, ACK delimiter, EOF and dominant-bit errors. 49: local busy, remote busy, arbitration loss, error/event count. These are the reporting device's counters, not the bridge's UART diagnostics. |
| 4A..4C | Plant mode, stage 3 fire delay and staging selection; energy-use and savings-time registers (one word each). |
| 4D..50 | Selected mix pump hours, outdoor low and high (degH), available/enabled mix pump masks. |
| 53..55, 58 | Maximum heat, minimum cool and maximum cool room targets in setback order; floor cooling enable. |
| 5A, 5B, 62 | Selected tank pump hours; available/enabled tank pumps; received-message count (u32). 62 is registered in the declarative catalog, not the indexed 483 reader. |
| 71..7D, C0..D2 | [Snowmelt configuration, slab targets and diagnostics](tn4-frames.md#addressed-snowmelt-blocks). |
| 83..8A | Water temperature selection (boiler/mix/tank bits), tN2 link, backup source, heat pump loop mode, cooling tank target, floor dewpoint mode, minimum cooling tank target, second heat pump configuration. |
| 8B..92 | Selected heat pump source: type, switchover mode, return heat/cool limits, outdoor heat/cool limits, minimum compressor run/off times. 8D..92 have a u16 value after the selector. |
| 93..98 | Interstage delay, differential, backup delay, plant heat/cool mode, available/enabled pumps, selected heat pump hours. |
| 99..A2 | One u16 runtime in hours each: W1 heat/cool, W2 heat/cool, Y1 heat/cool, Y2 heat/cool, humidification, dehumidification. |
| A3..A8 | Humidification/dehumidification setup, their setback targets, heat pump cycles, cooling purge minutes, onboard humidity sensor enable. |
| A9..B0 | Five balance-point targets, balance-point schedule ID, W1 shutdown, W2 lockout, Y during W2, baseload mode, unoccupied humidity enable and humidity differential. |

## Thermostat Sensor Inputs

Block `0D` assigns onboard and external inputs 1..4, one support bit
per slot. On an identified thermostat the codes are 0 off, 1 room,
2 floor, 3 remote, 4 outdoor, 5 coil, 6 duct, 7 setpoint, 8 hot tub,
9 DHW, and 10 pool. Block `15` reports room average, minimum and maximum;
`16..1A` report each input's own role and current, minimum and maximum.
The words are little-endian degH: Fahrenheit = (raw - 850) / 10.
Keep zero, reserved values and half-supported words out of numeric
readings. Two slots assigned the same role are still two inputs.

The 483 SW 11 inbound block fixups and thermostat model distinguish:

| Model / revision | Block | Interpretation |
| --- | --- | --- |
| 544 A..I; 545 and 546 A..J | `16` onboard only | Swap status codes 0 and 1. Other roles, setup and external inputs are unchanged. |
| 538 A..D | `17` external 1 | Ignore the unreliable role byte. A complete current word below 270 degH implies off; at least 270 implies floor. A missing or reserved word cannot establish a working sensor. |
| 528 A..B; 538 A..E | `17` external 1 | A status of off with a nonzero usable current can be displayed as floor. This applies only to the current reading, not the extrema or the floor assignment. |

These are model/revision-specific rules, not temperature limits. When a
revision is unknown, do not guess a correction. Preserve the wire role,
setup role and words alongside their interpretation. If setup and a known
status disagree, withhold labeled temperatures. The floor mean uses the
assigned floor inputs (external 1 alone on 528/538) and rounds down in
native units; do not publish it with missing contributors. The input
blocks are read-only here. Raw forwarding never changes their bytes.

### Thermostat equipment setup

The 483 SW 11 `StageSetup` block (`01`) has five bytes: first terminal
code, first system-pump byte, second terminal code, second system-pump
byte, and second-stage heat-source code. Both pump bytes declare a
7-bit delay and a required bit in bit 7; the indexed reader omits
the second required bit but the declarative layout names it. Keep the
terminal, source, and delay values in wire units. These are configured
outputs and pump requirements, not live relay states. A physical 553
reported all five fields.

`04` gives the number of cooling stages, minimum run and off times,
cycles per hour, interlock, and priority. The same 553 supplied the stage
count, interlock and priority, but did not support the other bytes even
though the positional report carried placeholders. `97` carries separate
available and enabled heat-pump masks, not counts. `08` carries packed fan
operation and ventilation settings, coil limits, delay, and three purge
settings. The physical 553 supplied the two packed bytes and fan purge;
the other fields were unsupported. `58` reports the floor cooling setting
(0 off, 1 on) on the cataloged 552. It also returned a supported byte on an
addressed read of the physical 553, even though SW 11 does not list it for
that model.
[Service Bulletin 082](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/43143-source/638820700500000000/sb0082-01.pdf)
describes the option's removal from later 552s and changes to floor cooling
on the 553 and 557; the bulletin alone does not establish a wire layout
for the 557. Do not infer a running stage or use a reported
floor-cooling option as proof of compressor capability. These blocks
have no applicable 483 field editor. Unknown values remain raw.

### Thermostat equipment monitors

| Block (hex) | Readback |
| --- | --- |
| `0A` | Filter replacement interval and time since replacement, as two little-endian u16 words. These are separate from total fan runtime. |
| `1B` | One u8 each for heat stages 1 and 2, cool stages 1 and 2, and fans 1 and 2. Keep the reported octets; they are not the air-group demand bits or the device's physical I/O directory. |
| `1C..21` | One little-endian u16 accumulator per output, in the same order as `1B`. The older 483 layout calls these accumulated time. Keep the raw word. |
| `98` | Heat-pump selector u8 and hours u16. Select instances using the available mask in `97` where supplied, not the enabled mask. |
| `99..A2` | One u16 hours counter each for W1 heat, W1 cool, W2 heat, W2 cool, Y1 heat, Y1 cool, Y2 heat, Y2 cool, humidification and dehumidification. These are not aliases of the `1C..21` stage counters. |
| `A8` | Internal humidity sensor selection (0 off, 1 on). Not a humidity sample or an `A3` humidifier/dehumidifier enable setting. |

The [557 user manual](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/81749-source/557-ul-01)
describes distinct terminal operating hours and air-filter maintenance.
An addressed scan of a physical 553 returned `0A` and `A8`, and four
supported relay fields in `1B`: both heat stages, first cool and first fan.
`99..9E` and `A1/A2` returned both bytes. `9F`, `A0` and the older `1C`
answered with zero masks. An advertised counter can still be zero. Its
presence says nothing about whether that output is wired or operating.
Read counters only when needed, and do not infer deltas, rollover, or
active outputs from incomplete or unrelated reports. No filter or sensor
selection edit is established in the 483 writers.

### Weather and equipment operating settings

Blocks `0B`, `0C` and `A9` hold configured warm-weather shutdown,
cold-weather shutdown and heat-pump balance points. Their five slots are
Wake, Unoccupied, Occupied, Sleep and Away. Block `0B` precedes the slots
with a separate source byte. Honor the support mask in either report
layout. These are not the live flags in `30F` WarmWeatherShutdown.

| Block (hex) | Configuration octet |
| --- | --- |
| A6, A7 | Heat-pump cycle setting (raw); cooling purge minutes. |
| AA, AB, AC | Balance-point schedule ID; W1 warm shutdown; W2 lockout. |
| AD, AE | Compressor operation during W2 backup; baseload mode. |
| AF, B0 | Unoccupied humidity enable; humidity differential. |

The 483 readers define the byte positions but do not establish the
threshold units, Off sentinel, schedule numbering or mode encodings.
Keep raw bytes, even zero or FF. No matching field writers are known.
An addressed read of a physical 553 returned supported fields in `0B`,
`0C`, `A7` and `A9..AF`. Only Wake and Sleep were supported in the `0B`
and `A9` tables, and only Wake in `0C`. `A6` reported mask zero and `B0`
did not answer. The model's scan tries the confirmed blocks; do not
infer the same availability on a different model or revision.

## Selecting Instances

A single `2D7` request can select several nodes on the same subnet. Each
responds separately with `2E7`; one link ACK is not evidence that all
selected devices replied. Match the source subnet, node (without its lock
bit), block ID and, for an indexed block, the supported selector byte.
An unindexed report with a zero support mask is a reply. An indexed report
without its selector cannot establish which instance replied.

The 483 SW 11 device, timer and reset-control models give these read hints:

- Timer 033 uses wire selectors 0..3 for each of blocks `38..3B`. Its UI
  schedule IDs 2..5 are not the block arguments.
- 400/401/402 use boiler 0 and pump IDs 0 and 129; 406 uses pump 0; other
  reset controls use pump IDs 0 and 1. For boiler-specific blocks on other
  models, use set-bit positions in the **available** boiler mask in `35`,
  not the enabled mask or a count. For tank pump hours use `5B`.
- For reset-control bus counters (`48/49`), SW 11 uses water bus minus one,
  not the bridge's physical port. Water bus zero supplies no such index.
  The base 483 device model does not enumerate ports for these blocks.
  For other models, including the 654, use a selector from an actual
  report or one explicitly supplied by the caller, not an assumed zero.
- The mix/heat pump availability masks (`50/97`) and matching runtime IDs
  (`4D/98`) likewise provide candidate pump indices. The returned selector
  and support mask confirm what the device actually supplied.

For other indexed blocks without an enumeration, zero is a first probe,
not a maximum.
Keep other selectors actually seen from that device. A present zero
availability mask advertises no instances; a missing mask is unknown.

### Device link counters

These addressed registers are independent of the 482's gateway lifetime
counters. `48` reports a device-local port ID followed by seven u8 counts:
stuffing, CRC, missing CRC delimiter, missing ACK, missing ACK delimiter,
missing EOF and dominant bit. `49` reports a port ID followed by four u8
counts: local busy, remote busy, lost arbitration and errors/events.
Keep the receiving gateway port and the reported port ID separate.
The SW 11 declarative registry also defines `62` as a little-endian u32
received-message count; no model read table establishes that a device
supports it. Preserve the support mask and raw counter values. Neither the
reset/wrap behavior nor the use of these registers on a particular model
is implied by the field layouts. No writes are defined for these blocks.

## Addressed Writes

A `2DF` body contains a destination subnet, a five-bit destination node
(the report's lock flag is not copied), a little-endian block ID and the
positional data bytes. There is **no field mask in an update**. An edit
starts with `2D7` and a matching `2E7` from that port/subnet/node and
instance. Copy the whole known positional image, change only the selected
supported field bits, send once, then issue a new read. A link ACK does
not confirm application, nor does a timeout authorize repeating the update.
No protocol operation locks the block against edits by a second bus writer.

The [field edit catalog](../tools/payloads/writes.py) covers all 54 explicit
`BlockTypesIn.write` definitions in the 483 SW 11 software. These are
known update encodings, **not** a promise every model accepts every edit.

| Blocks (hex) | Editable data |
| --- | --- |
| 03, 05..07, 09 | Heating mode/CPH/air group; heat, cool, floor and ventilation targets in setback order. Floor also has a maximum. |
| 0E, 0F | Scene settings, schedule IDs, cycling rate, daily event selection and optimum start. The 483 writer always sets the sixth schedule-setup byte to zero, even though its reader calls it RTC setback enable. Do not offer that byte as editable or clear it as a side effect when it is nonzero. |
| 10..13, 38..3B | Seven weekdays per event; multi-schedule also packs master, cycling and event-count flags with the selector. Preserve the low three selector bits when editing these flags. Day byte 144 means disabled. |
| 1C..21, 43, 44, 4C, 98..A2 | Relay, boiler, pump and heat-pump hours, savings-time and output counters. Indexed runtimes keep their complete selector byte. Counter updates can destroy operating history. |
| 24..27 | DHW and setpoint exchange, application and operating setup. Unsupported placeholders can be nonzero and must survive unchanged. |
| 28..31 | Setpoint, hot-tub, DHW, floor and pool targets. Use whole little-endian words. The six-byte `2E` floor variant omits maximum; do not extend it to eight bytes. |
| 71, 72, 78, A4, A5 | Melt, idle, storm slab targets and humidification/dehumidification setback targets. Changing a slab target does not start a melt. |

There is one additional model-level reset: SW 11's reset control sets the
indexed `5A` TankUsage hours to zero. Its declarative layout is one tank
ID and a little-endian word. The indexed Ruby reader has no standalone
writer for this block; arbitrary replacements are not established.

Reject ambiguous, incomplete or extended source images before changing a
setting. In particular, a compact sparse report may be readable but has
no bytes to preserve for the missing positions. All bytes of a selected
word must be supported. On readback, an unambiguous compact report can
confirm selected fields without requiring unchanged unrelated fields.
The stock 482 also has a four-data-byte `0D` sensor-setup emitter branch;
that shorter trace layout is decoded, not treated as a general sensor editor.

### Scene response settings

Block `0E` applies to one addressed device. In positional order it contains
the away period, hold period, responses for scenes **1, 4, 5, 7, 8**, and
the scene participation setting (0 disabled, 1 enabled). The 483
`SceneResponse` codes are 0 Wake, 1 Unoccupied, 2 Occupied, 3 Sleep,
4 Away, 5 Schedule. They are not tHA setback IDs. Unknown values stay raw.
The other three scenes have no writable response slot.

Use only supported report bytes; limited devices can omit response slots
or set participation through a physical switch. To edit a response,
preserve the other seven positional bytes and verify the specific field
with a second read. Changing a response does not select that scene, and
reading this block does not return the active scene.

## Reading a 402

With capability bit 10, use [NormalBlockRead](tpck-bridge.md#addressed-normal-reads)
for the identification block or any addressed setup block. It returns the
peer's actual frame without enabling trace. Supply the selector for an
indexed register; the block catalog describes possible reads, not which
instances the particular control has installed.

In Normal mode, capability bit 4 advertises `NormalIdentity` (tRPC
`0x544BF00D`). Request with three bytes: physical port (0..4), full
subnet, and block destination node (1..31). The ResponseRequest echoes
these bytes when queued. Busy rejects a second pending read on that port.
This schedules only a BlockRequest for block 00, never a write, mode
switch or inventory reset. Watch Normal trace on that port for the
BlockReport from the actual responding source. Acceptance and link ACK
are not model identification. Sources with no addressable node or no
usable reply cannot be assigned a model by this request.

Use block 00 to confirm the native source. A plant master need not have
a thermostat-style tHA inventory address. To locate it, send MasterQuery
on one physical port and use the **source of its MasterPresent reply** for
the next block read. The boiler and mix subnet bytes inside that report
describe loops; they are not interchangeable addresses for the master's
registers. A read of the same pump-hours register on the same port can
time out at an advertised loop address and succeed at the reporting source.
On a 402, read-only queries
confirmed distinct `44` replies for primary pump selector 0 and DHW pump
selector 129. Both echoed the complete selector byte followed by the
hours word (mask `07`). Boiler and mix pump hours used selector 0 on
`43` and `4D`. Blocks `41`, `24` and `25` also returned positional data
with gaps. Use the mask in the packet, not a model spreadsheet, to
decide which fields were supplied.

On this model `4B` is an hours-based boiler energy-use register, not a
kWh or BTU reading. Multiply it by the boiler input rating in BTU/hour
to estimate BTU. `4C` measures hours in the savings state. The
[402 installation manual, Monitor menu (page 19)](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/81499-source/638820695310000000/402-d-07.pdf#page=19)
explains these readings and the separate boiler and pump counters.
