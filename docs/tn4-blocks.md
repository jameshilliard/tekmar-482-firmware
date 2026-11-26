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
It does not infer a model from a J number, a group ID or a master role.
Two early thermostat variants share a product number; so do a power
manager and a user switch. Without a distinguishing J number they
remain unidentified. For identified models, the 40 SW 11 read tables
suggest blocks to try; the masks in actual reports establish the
available data, not these lists.

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
- The mix/heat pump availability masks (`50/97`) and matching runtime IDs
  (`4D/98`) likewise provide candidate pump indices. The returned selector
  and support mask confirm what the device actually supplied.

When no enumeration is known, zero is a useful first probe, not a maximum.
Keep other selectors actually seen from that device. A present zero
availability mask advertises no instances; a missing mask is unknown.

## Reading a 402

Use block 00 to confirm the native source. A plant master need not have
a thermostat-style tHA inventory address. On a 402, read-only queries
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
