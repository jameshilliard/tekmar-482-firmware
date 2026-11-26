# Normal-mode compatibility

The [published tHA protocol](https://www.watts.com/our-story/brands/tekmar/tha)
defines the host-visible operations. The 482 CS 02 sample supplies the packed
field layouts; the 482 SW 06 J1196J handlers and the native
[frame catalog](tn4-frames.md) connect them to bus behavior. Recognizing a
native message in Raw mode does not implement its tRPC services in Normal mode.

| Method IDs (hex) | Normal-mode behavior in this firmware |
| --- | --- |
| 000 NullMethod | Reply to an unknown method on the matching response service. |
| 18F ProtocolVersion | Request returns 3; this is the wire format, not a coverage flag. |
| 167 DeviceInventory | Full walk, exact lookup, clear one, clear all. No unsolicited inventory report. |
| 197 DeviceType, 19F DeviceVersion | Read addressed identity from block 0. Ignore writes. |
| 15F TakingAddress | Native address changes update the cache. Serial report not yet implemented. |
| 107 NetworkError | Request and Update return the current network error word. Reports follow native 3D7, including clearance. |
| 10F ReportingState | Request and Update. Power-on default comes from the Reporting DIP. Test-button reporting notifications are not yet implemented. |
| 117 OutdoorTemp, 11F DeviceAttributes | Not implemented. |
| 127 ModeSetting | Addressed Request and Update use heating setup (block 3). Report on observed mode. This is not ActiveDemand. |
| 12F ActiveDemand, 137 CurrentTemperature, 138 CurrentFloorTemperature | Request, read-only Update echo and Report. Source and availability rules below. |
| 13D SetpointGroupEnable, 13E SetpointDevice | Not implemented. The documented melt/storm controls are group operations, not discovered device addresses. |
| 13F HeatSetpoint, 147 CoolSetpoint | Addressed Request and Update by setback, using native blocks 5 and 6. Report the active slot when a block is observed and the active setback is known. |
| 14F SlabSetpoint, 157 FanPercent | Addressed Request, Update and active-slot Report. Blocks 7 and 9; see sensor requirements and fan units below. |
| 150 RelativeHumidity | Request and Report from RHReport. Update is ignored. |
| 151 HumidityMax, 152 HumidityMin | Addressed Request, Update and Report. Require an RH source and the corresponding block A3 enable flag. Not setback-indexed in tHA. |
| 17F SetbackEvents | Not implemented. |
| 16F SetbackEnable, 177 SetbackState | Read or set the gateway enable flag; read the active setback and receive setback reports when enabled. The flag also determines which heat, cool, floor and fan slots an Update changes. |
| 187 FirmwareRevision, 1A7 DateTime | Not implemented. FirmwareBuild is a separate custom extension, not a vendor revision. |

## Inventory and identity

A valid status or block report from a thermostat node (1..24) on the port's
subnet establishes presence. Preserve the physical port when converting to
the decimal tHA address. A master subnet change invalidates the old port
entries. Addressed devices age out after 150 seconds without qualifying
traffic. A link ACK, a bus master or a group broadcast is not a thermostat.

Request 167 accepts a little-endian u16. At zero the gateway snapshots the
table and streams a separate response for each address followed by exactly
one zero. An exact lookup returns just the address, or FFFF if absent.
Update 167 deletes one entry (zero deletes all) and echoes the address or
FFFF. This changes only the gateway cache; later traffic rebuilds it.

For 197 and 19F send the two-byte address, optionally followed by the
four placeholder bytes in the vendor examples. A present device returns
its address and a u32: the product number or J number times 100 plus Z.
FFFFFFFF means not yet known. For an absent address neither method replies;
use an exact inventory lookup to establish absence. A missing device must
never return uninitialized version bytes.

Normal mode issues an addressed, read-only native block-0 request on
qualifying traffic when identity is incomplete and that port is available.
Repeated observations of the same device are rate-limited to one probe per
minute. Raw mode does not issue these requests, cache identities or expose
internal probes as host transmit results.

## Live readings

Air-group member (36F), master (377), and setpoint control (22F, operating
mode 1) use different offsets for measured temperature and demand. A device
in heating mode with no demand reports 0, not 1. The master reports heat if
both heat and cool are demanding. A native RHReport (323) contributes its
control humidity, not a humidity target.

SlabStatus (26F) supplies the floor reading. The gateway requests sensor
setup (block 000D) to determine whether that reading also supplies
CurrentTemperature. Only one floor sensor and four off slots qualifies as
floor-only. A mixed room/floor configuration keeps both readings separate.
No confirmed floor configuration means the floor reading is unavailable.
Native temperature sentinels 4000, 8000 and C000 all become FFFF in tRPC.

The gateway updates its cache even when reporting is off. When enabled,
qualifying native reports schedule serial reports immediately; the gateway
does not wait for a periodic poll or synthesize a sample on a quiet bus.
If the UART is busy, pending samples of the same device and field collapse
to the latest value. Disabling reporting clears pending samples; explicit
reads still work. SetbackEnable gates setback reports, not temperature or
demand. No telemetry Update writes a thermostat setting.

## Thermostat settings

Normal mode accepts one native setting operation per physical port. A
Request sends an addressed 2D7 and waits for a matching 2E7 from the same
port, subnet and node. An Update first does the same read. For mode it
changes byte 0, preserving heating cycle rate and air-group ID. For heat
and cool, the five positional bytes are the native setback values. With
setbacks enabled, selectors 0/2/4 read byte 0 and write bytes 0 and 2;
1/3/5 read byte 1 and write bytes 1 and 3; Away (6) uses byte 4. With
setbacks disabled a write changes all supported slots. CURRENT (7) uses
the last observed active setback; an unknown active slot is not guessed.

Slab and fan use the same slot aliases as heat and cool. For slab, the
gateway first reads sensor setup (000D) and requires a configured floor
sensor. Block 7 byte 5 is the floor maximum, not another setback slot;
all writes preserve it. SlabStatus type 0/1 can report the active target;
other types do not supply that value. A floor-only setup is not evidence
for a room heating or cooling setpoint.

FanDuctStatus (25F) describes fan operation, not the configured target.
It prompts a rate-limited read of ventilation requirements (block 9).
The tHA fan value stays in device units: 0 is Auto; full scale is 10 on
models 544/545/546 and 100 on other documented models.

RHReport (323) establishes a humidity source. For a humidity limit, the
gateway reads setup (A3) before reading the target. Humidification must
be enabled in byte 0 for Min (A4), dehumidification in byte 1 for Max
(A5). Missing or unavailable setup values do not enable either control.
tHA reads and edits only byte 0 in each five-byte target block, regardless
of SetbackEnable. The remaining native targets are preserved. Normal mode
also probes enabled limits on RH activity for push reports.

An Update needs a complete fixed-width block: the gateway will not invent
missing siblings or discard an extension. It preserves unsupported bytes.
If the selected bytes already match, it replies from that fresh read.
Otherwise it sends one native 2DF Update, then requests the block again.
Response:Update contains the value actually read back, including when the
device kept the old value. A link ACK is not application confirmation.
There is no automatic write retry or atomic lock against another bus writer.

An unsupported field, incomplete write layout, lost transaction or expiry
returns FF for the value, not the requested setting. After a failed update
the on-device state may be unknown. Read it before deciding whether to try
again. The transaction budget is ten seconds; allow more than that on the
host to receive a negative reply from a quiet device. An overlapping setting
command on the same port returns BridgeError (Busy). Raw mode is unaffected.
