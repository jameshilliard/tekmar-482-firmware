# Normal-mode compatibility

The [published tHA protocol](https://www.watts.com/our-story/brands/tekmar/tha)
defines the host-visible operations. The 482 CS 02 sample supplies the packed
field layouts; the 482 SW 06 J1196J handlers and the native
[frame catalog](tn4-frames.md) connect them to bus behavior. Recognizing a
native message in Raw mode does not implement its tRPC services in Normal mode.
Compatibility means that existing software can use the same framing, services,
addresses, payloads and units. Private stock task schedules and stale-cache
behavior are not part of that contract.

The J1196J input dispatcher at `0xae8c` has **26** named cases. All 26
are handled here. NullMethod is the fallback, and TakingAddress is an
additional output-only ID. Method coverage does not imply identical bus
behavior or compatibility with every serial client's timeout.

| Method IDs (hex) | Normal-mode behavior in this firmware |
| --- | --- |
| 000 NullMethod | Reply to an unknown method on the matching response service. |
| 187 FirmwareRevision, 18F ProtocolVersion | Request returns a little-endian u16; Update is silent. The physical Test button reports both. |
| 167 DeviceInventory | Full walk, exact lookup, clear one, clear all. No unsolicited inventory report. |
| 197 DeviceType, 19F DeviceVersion | Read addressed identity from supported fields of block 0. Apply the documented 537/541/542/161/162 [legacy corrections](tn4-blocks.md#discovering-identities); stock J1196J at `0x6918` just copies the product and computes J x 100 + Z. Ignore writes. |
| 15F TakingAddress | Report the old and new addresses even with reporting disabled. The stock input dispatcher has no case for this report ID: host Request and Update get an empty NullMethod on the corresponding response service. |
| 107 NetworkError | Request and read-only Update return the most recently received fault word. With reporting enabled, forward each native 3D7, including repeated values and clears. No source address in tHA. |
| 10F ReportingState | Request and Update read back the active reporting flag. A Test-mode Report carries the physical DIP position instead. |
| 117 OutdoorTemp | Read the selected network temperature, supply a four-minute local sample, and report changes. Respond to native outdoor requests on all five ports. |
| 11F DeviceAttributes | Read the observed u16 mask, echo it on Update, and report changes. See below for the evidence behind each bit. |
| 127 ModeSetting | Thermostats use heating setup (block 3). For a recognized setpoint controller, Request and Update return FF without a native read or write. No thermostat mode Report follows. This is not ActiveDemand. |
| 12F ActiveDemand, 137 CurrentTemperature, 138 CurrentFloorTemperature | Request, read-only Update echo and Report. Source and availability rules below. |
| 13D SetpointGroupEnable | Request cached state for group 1..12; Update broadcasts on all five lines. Report from observed group status. |
| 13E SetpointDevice | Addressed Request and Update of a 16-bit target by setback. Reports from setpoint control status, not group enable. |
| 13F HeatSetpoint, 147 CoolSetpoint | Addressed Request and Update by setback, using native blocks 5 and 6. Report the active slot from configuration or air-group status (36F/377). |
| 14F SlabSetpoint, 157 FanPercent | Addressed Request, Update and active-slot Report. Blocks 7 and 9; see sensor requirements and fan units below. |
| 150 RelativeHumidity | Request and Report from RHReport. Update is ignored. |
| 151 HumidityMax, 152 HumidityMin | Addressed Request, Update and Report. Require an RH source and the corresponding block A3 enable flag. Not setback-indexed in tHA. |
| 17F SetbackEvents | Request, read-only Update echo and Report. An inventory entry starts at 0. Each native 2F7 replaces it with bits 7:6 of byte 0: 0/1/2 for zero/two/four events, 3 reserved. Unknown addresses do not reply. |
| 16F SetbackEnable, 177 SetbackState | Read or set the gateway enable flag; read the active setback and receive setback reports when enabled. The flag also determines which heat, cool, floor and fan slots an Update changes. |
| 1A7 DateTime | Update broadcasts local wall time on all five lines. No Request response or Report. |

**Snow control schedules.** The
[654 manual, p. 31](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/81803-source/638820695340000000/654-d-03.pdf#page=31)
describes two or four events and master, member and local zone roles.
The published 482 protocol promises `SetbackEvents` only for thermostat
masters. On the wire, J1196J initializes the addressed cache byte to
zero on allocation (slot + 0x33, record + 0x2f, at `0x2450`). Its
`0x6538` handler stores the upper two native bits without checking the
master bit or the product. `0xa610` / `0xa5b0` return that byte unchanged;
an Update cannot set it. This firmware uses the same rule, including for
the 654. A zero reply alone does not distinguish no schedule from no
announcement yet; the code alone never proves schedule ownership. Use the
native role and schedule setup for that.

## Bus compatibility

Normal mode acts as a tN4 receiver on all five lines. After a valid CRC
and delimiter it drives the one-bit link ACK, including while its own
transmission is waiting for the line to become idle. It does not ACK its
own frame. A link ACK is independent of serial reporting, application
support for the message, and whether the device applied a setting.
Raw mode is passive unless the host explicitly selects ports for RX ACK.

Normal mode runs discovery independently on all five physical lines,
even with serial reporting disabled. The stock application service
(`0x78dc`) samples link state once per second:

- At startup an unseen, quiet port gets one empty class-1 NetworkInitRequest
  (`3DF`). The stock startup table at `0xc458` zeroes both the seen flag
  and its deadline; `0xb410` treats that deadline as expired.
- On a sampled transition to link-up, send one empty class-1 MasterQuery
  (`3F7`). Start a new 30-second window. Unless a peer sends `3DF` on
  **that port**, send `3DF` once at the end of the window. MasterPresent
  (`3FF`) supplies the routing subnet **in its header**, even if its
  payload is empty (`0x597c`); it does not suppress the follow-up. An
  empty payload supplies no boiler or mixing information.
- A valid received frame or an externally ACKed send establishes link-up.
  A gateway's unacknowledged echo is not a peer. A link fault followed by
  recovery can start a new cycle; elapsed silence alone cannot. Neither
  a probe nor an ACK creates an inventory entry.

Host setting operations and broadcasts take precedence over a discovery
frame that has not started. Switching to Raw cancels pending probes;
returning to Normal starts fresh. Raw never initiates line discovery.

**Arbitration is not a missing ACK.** In J1196J, if a peer drives a
dominant bit over a recessive data or CRC bit, the gateway switches to
receive (`0x4668`). The send stays pending at the winner's interframe
gap and can contend again. If instead the gateway finished the frame
but got no ACK, recovery clears the pending send on a quiet gap
(`0x4838`, `0x4cb8`, `0x42d4`). It does not send a second copy.
Normal mode likewise yields to the winner, receives and ACKs its valid
frame, then checks whether the pending operation is still needed.
Raw returns one terminal result per host send, including arbitration loss.

The per-line error score is shared (`0x4354`): a missing ACK adds 16,
and a second missing ACK marks the link down. A framing error adds one;
if the resulting score is at least ten, new sends on that line wait for
about one second of **idle time** (1,603 idle timer matches at 624 us).
A dominant first interframe bit also starts this backoff. Valid traffic
can still be received and ACKed; it does not pay down or cancel the wait.
Other lines continue to operate.

**Active error recovery is per port.** In Normal mode a stuffing or CRC
error, a dominant delimiter, a dominant bit in EOF or the first interframe
bit, or failure to see a locally driven dominant bit triggers an error flag.
Only the *sender* flags a missing ACK; hearing another sender go
unacknowledged is not itself a receive fault. As in J1196J
(`0x42b8`, `0x4354`, `0x4838`), start at the next bit boundary,
drive six dominant bits, release, and wait for eight **consecutive**
recessive bits followed by the three-bit interframe gap. Overlapping
flags restart the quiet count. The gateway does not transmit a late
flag if it misses the drive window. Raw never generates an error flag,
including when receive ACKs are enabled; it reports malformed input
and send outcomes to the host instead.

Stock settings often reply from cache. This implementation can answer a
configured-mode read from peer-confirmed state. Setting writes always start
with a native read and return the value the device actually retained.

**Targeted reads.** Status frames queue reads for missing identity, mode,
setpoints or setup on the source port. Repeated requests for the same device
and block occupy one entry. A busy port retains its reads; another port can
make progress independently. A matching block report immediately releases
that line's next read. If the peer doesn't answer, the line allows another
background read after one second. This window is not a delay on serial
reports, explicit host operations or verified write readback.

A change in air group, heat/cool enablement, demand, setback or the active
target justifies re-reading configured mode, not guessing its value. An
idle thermostat can enable heat without calling for heat. A member frame
describes only its own channel. Compare its mode hints with the previous
sample for that channel in the same group and role. Unchanged heat and
cool reports can alternate without invalidating the confirmed mode, even
when their demand or setback differs. The tHA ActiveDemand and SetbackState
values still come from the last report, as on the stock gateway.
A change hint for a known mode gets a priority turn in that port's read
queue. Pending reads for the same node coalesce, and regular reads still
get turns while multiple thermostats are changing.
A temperature-only change does not
trigger another mode read while the mode sample is fresh. No inventory sweep
runs on a quiet bus. J1196J independently walks one device per minute
(`0x76c4`), spaces block requests globally (`0x7538`), and suppresses
optional reads for two minutes after WaterCycleStart (`0x68e4`). Those are
stock cache policies, not tHA operations. This firmware keeps the native
arbitration and error recovery without imposing those cross-port delays.

**ModeSetting reads.** J1196J reads the stored mode byte at device record
offset 0x11 (0x93b4 -> 0x9338 -> 0x27dc). In this firmware a Request can
reply immediately if the same thermostat has confirmed block 03 in the
last five minutes. A change hint, an addressed heating-setup write or an
address change invalidates the sample. Neither a write's baseline nor a
link ACK confirms its outcome. Use NormalBlockRead to explicitly request
fresh native evidence regardless of cache age.

Requests for an unconfirmed mode share a targeted read. They do not reserve
the port; a silent thermostat cannot hold another thermostat's confirmed
reply. Every accepted Request gets its own Response:Request, including when
reporting is disabled. Joined reads share the first request's ten-second
deadline. If the queue is full the gateway returns Busy instead of silently
dropping a reply. On timeout it returns FF and queues one read-only recovery
for the same device. Later peer evidence restores subscribers without
replaying a write.

Enabling reporting publishes current confirmed modes once. Subsequent mode
changes take precedence over routine readings for that thermostat but do not
overtake its older explicit mode replies. Unchanged setup blocks don't
produce duplicate Reports.

**Live room setpoints.** An eligible air-group status updates the target
cache directly. A member supplies its enabled side; a master with both
sides enabled supplies the calling side (heat wins a tie; neither if idle).
The master's heat-side flags supply the setback for **both** sides.
Zero, channel flag bits 6/7, or a matching group inhibit code prevent a new
sample. Groups 0..16 share their last master code across ports: 1 inhibits
heat, 2 inhibits cool; J1196J compares for equality, not as a bitmask.
Other groups use code 0. FF is an explicit unavailable value.

With setbacks enabled, a sample changes the occupied alias pair, unoccupied
alias pair, or Away slot; with setbacks disabled it fills all five. Reports
use the current selector and cached value (FF for unknown), even when the
latest frame did not supply a new target. Mode Off suppresses both, Heat
suppresses cool, and Cool suppresses heat. Floor-only and native setpoint
devices do not emit room target reports. A status is not device readback
for an outstanding setting write.

The stock **native application dispatcher** at `0x6d98` is a separate
check from the 26 serial methods. Its cases are:

| Native IDs (hex) | Normal-mode coverage |
| --- | --- |
| 20F, 22F | Setpoint group enable and addressed setpoint controller status. These are independent. |
| 25F, 26F, 323, 36F, 377 | Fan, slab, humidity and air-group member/master observations. |
| 2E7, 2F7 | Supported configuration blocks and schedule status; setting readback and cache timing differ as described here. |
| 327, 32F | Outdoor temperature request and source report. |
| 37F | WaterCycleStart is native plant telemetry, not a serial tHA method. Raw forwards it; Normal does not suspend unrelated reads. |
| 3D7, 3DF, 3E7, 3FF | Network error, network initialization, address move and master/subnet presence. |

A Raw decoder for an additional 483 message is not evidence that the stock
482 exposes it over tHA. Conversely, exercising every dispatcher ID is
not a timing or electrical equivalence test on real controllers.

## Gateway diagnostics

NetworkError preserves both native bytes unchanged. Incoming faults and
clears are queued in receive order, not coalesced like temperatures.
The gateway buffers 128 pending fault reports; if the UART falls further
behind it discards the oldest pending report first. Disabling reporting
discards undelivered reports, not the last word returned by a Request.
A zero code clears the **sender's** fault. Since tHA omits the sender, a
client cannot infer that every device is clear from the latest word.
Use [Raw fault reports](tn4-frames.md#network-faults) for per-source state.

FirmwareRevision returns 0 for this unnumbered custom firmware. This is a
project convention, not a tekmar revision or a reliable way to identify a
custom image. Use BridgeCapabilities and FirmwareBuild for identification
and updates. ProtocolVersion returns 3, the tHA wire format.

Pressing the physical Test button reports FirmwareRevision immediately,
then ProtocolVersion ten seconds later. Both stages last ten seconds with
the Test LED on. Press during either stage to hold it (Test blinks every
500 ms); press again to advance. A held stage times out to idle after five
minutes. After stage two there is a three-second lockout with the LED off.
These are serial diagnostics; they do not put any tN4 device into test mode.

During either **unheld** stage the gateway samples the physical Reporting
DIP once per second and sends a ReportingState Report if it differs from
the last reported position (initially off). Like J1196J at `0x78dc`, this
does not read or change the software flag. All three Test reports work
with ordinary reporting off; a ReportingState Request still reads the
software flag. Raw mode sends none of these reports, and a mode transition
clears the test sequence and any pending notices.

## Clock updates

DateTime accepts a little-endian year (2000..2255), month (1..12), day
(1..31), ISO weekday (Monday=1 through Sunday=7), hour (0..23) and minute
(0..59). Like J1196J at `0xac90`, it checks field ranges, not calendar
consistency. An Update echoes these seven bytes when admitted. Invalid
fields or a nonempty native transmit queue return seven FF bytes on
Response:Update. A frame already handed to the line driver is not queued;
waiting for a serial reply is not queued either.

Each line gets class-1 native 31F with five bytes: minute, weekday OR
(hour << 3), month minus one, day minus one, year minus 2000. There is
no DST-mode byte. The echo acknowledges queue admission only, not delivery
or a change to any particular thermostat. ReportingState does not gate the
write. Normal mode neither reads back the clock nor reports observed native
clock frames; those remain available in Raw mode.

## Inventory and identity

A qualifying status or block report from an addressed node (1..24) on the port's
subnet establishes presence. Preserve the physical port when converting to
the decimal tHA address. A master subnet change invalidates the old port
entries. Addressed devices age out after 150 seconds without qualifying
traffic. A link ACK, a bus master or a group broadcast is not a thermostat.

A native snowmelt status (17E) with a valid zone or sensor number and at
least one complete declared key/value pair also establishes and renews
presence. Use the header's subnet and node, not the configured zone or
sensor number in the payload. This triggers the same addressed block-zero
identity read as other qualifying traffic. A truncated frame, a node-zero
or master-node report, or a report on another subnet cannot refresh that
entry. Snowmelt status does not populate tHA thermostat readings. Use
NormalStatusRead or Normal trace to read the snowmelt words themselves.

Native 3E7 identifies the previous wire subnet and node in its payload
and the new address in its header. Like J1196J, emit the serial report
only for a known previous address. Move its identity to the new address
and cancel pending setting operations for either affected device. A
repeated move from an already-vacated address does not erase that identity.
Later addressed operations use the new wire subnet, not its three-bit
display value. Moves are queued in arrival order independently of sampled
telemetry. The queue holds 128 moves; under sustained UART overload the
oldest unforwarded move is dropped. An inventory walk reads the current map.

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

Qualifying traffic queues an addressed, read-only native block-0 request
when identity is incomplete. Normal mode can retain it while the port is
busy. Repeated observations of the same device trigger at most one identity
probe per minute. Raw mode does not issue these requests, cache identities
or expose internal probes as host transmit results.

## Device attributes

Read 11F with a device address. A present device with no capability evidence
returns zero; an absent device does not reply. An Update ignores the supplied
mask and returns the real one. Neither operation transmits on tN4. The first
four bits are published in the tHA reference; J1196J also uses bits 4..8.
Product identity alone does not set bits.

| Mask (hex) | Native evidence |
| --- | --- |
| 0001, 0002 | Air-group master (377) establishes heat and cool. Member (36F) establishes heat, and cool when byte 1 bit 0 is set and byte 0 bit 5 selects cooling. A floor-only configuration suppresses both. |
| 0004, 0100 | Sensor setup (block 0D) has exactly one floor sensor and four off slots (0004), or any floor sensor (0100). A floor-only device has both bits; a mixed room/floor device has only 0100 of these two. |
| 0008 | FanDuctStatus (25F) or a ventilation requirement in block 09. This says nothing about the current fan speed. |
| 0010 | RHReport (323), independent of whether either humidity limit is enabled. |
| 0020, 0040 | Following RHReport, block A3 enables the maximum via byte 1 and the minimum via byte 0. Off or unavailable flags do not grant access. |
| 0080 | Setpoint control status (22F) with operating mode 1, or a setpoint control setup/target block (26, 28, 2A, 2C, 2E, 30). This is not a setpoint group. |

The custom firmware reports a change to **any** mask bit with reporting
enabled, before dependent samples still pending for that device. Like the
stock gateway, it can repeat the mask on a slab status with a configured
floor sensor. The mask continues to accumulate evidence when reporting is off.

## Setpoint groups

SetpointGroupEnable is global, with a two-byte body: group ID and enable.
It does not take a thermostat address. The 654 defaults to group 1 for
melt and group 12 for storm; those are configurable, not model identifiers.
An Update with value 0 or 1 queues native 21F on each of the five lines.
Newer updates to the same group replace a copy that has not started on
that line. An already-started copy finishes before the next. Unstarted
copies expire after ten seconds. A copy that lost arbitration may contend
again if still current; a completed frame is never replayed for a missing ACK.
Invalid group IDs or enable bytes never cause a bus write.

As in J1196J, the gateway does not echo success for a valid Update.
Reporting must be enabled to receive a group Report. Only native 20F
updates the cache: byte 0 selects the group; bit 4 of byte 1 is the
**actual** enable, not bit 0 (requested enable). Link ACKs, outgoing
commands and cache expiry never generate a success report.

A Request reads the cache without transmitting. An active value expires
60 seconds after the last active status. An idle or never-seen valid group
reads 0; an invalid group reads FF. Neither zero nor expiry identifies
attached equipment. Raw mode preserves the physical port and native source
if attribution is needed.

## Live readings

Outdoor temperature is global. Request 117 returns the first little-endian
degH word from native 32F, or FFFF if neither the network nor the host has
a reading. Zero in a bus report means no network source in J1196J; native
temperature sentinels also do not override a valid host sample. A host
Update accepts 0 through 3049 (J1196J at `0x2230`); an invalid value does
not renew the previous lease. The response is the **selected** temperature,
which can differ from the supplied one. Refresh a local sample before 240
seconds have passed. Reporting enable gates serial push, not source selection.

The custom firmware selects a fresh external sensor first. Its silence
timeout is 660 seconds without a valid host sample, or 630 with one. With
only a host source, it broadcasts on a value change, and every 600 seconds
if the host continues renewing the same value. It does not count its own
transmission as an external sensor. Native 327 requests a snapshot on all
five lines, including FFFF when unavailable. The transmitted 32F contains
the word twice, as in J1196J at `0x3dd8`. Each unstarted broadcast has the
same ten-second queue lifetime as a demand-group send. No link ACK changes
the selected temperature.

Air-group member (36F), master (377), and setpoint control (22F, operating
mode 1) use different offsets for measured temperature and demand. A device
in heating mode with no demand reports 0, not 1. The master reports heat if
both heat and cool are demanding. A native RHReport (323) contributes its
control humidity, not a humidity target.

SlabStatus (26F) supplies the floor reading. The gateway requests sensor
setup (block 000D) to determine whether that reading also supplies
CurrentTemperature. Only one floor sensor and four off slots qualifies as
floor-only. A mixed room/floor configuration keeps both readings separate.
Truncated or ambiguous setup reports and reports without any supported
assignment leave the last confirmed configuration and pending reports intact.
No confirmed floor configuration means the floor reading is unavailable.
Native temperature sentinels 4000, 8000 and C000 all become FFFF in tRPC.

The gateway updates its cache even when reporting is off. When enabled,
qualifying native reports schedule serial reports immediately; the gateway
does not wait for a periodic poll or synthesize a sample on a quiet bus.
If the UART is busy, pending samples of the same device and field collapse
to the latest value. Disabling reporting clears pending samples; explicit
reads still work. SetbackEnable gates setback reports, not temperature or
demand. No telemetry Update writes a thermostat setting.

ScheduleStatus (2F7) describes a schedule, not necessarily the thermostat's
effective setback. A known device with the schedule-master flag (byte 0,
bit 2) advertises its event configuration in bits 7:6. Both the serial
packet and Raw mode preserve the code rather than a literal event count.
Do not interpret the two-bit native current-state field as a tHA setback.

## Thermostat settings

Setpoint controllers use a different layout from thermostat HeatSetpoint.
In native 22F operating mode 1, bytes 2..3 are the active target, not
the measured temperature (bytes 4..5). A Report carries the address, the
setback from that same status, and the little-endian target word. Like
J1196J at `0x66a4`, Normal mode passes the word through except for native
fault sentinels; it does not rescale it for a particular application.
Reporting does not require SetbackEnable. With an unknown active slot
there is no attributable target report.

Operating status also schedules CurrentFloorTemperature (FFFF without a
configured floor reading), not ModeSetting. Recognition withdraws pending
thermostat mode, heat and cool reports and prevents unstarted writes to
those thermostat blocks. Operating heat/cool direction is not a mode setting.
Operating status queues an addressed application setup read independently of
any missing identity fields. Setup does not depend on a block-0 reply.
Unanswered setup probes are limited to one per minute; a recognized
application remains fresh for five minutes.

For Raw consumers, the 483 SW 11 model for the 161/162 presents the **live**
target and temp1 as E for DHW (application 2), H otherwise. Do not assume
units without setup from that device, assign a role to temp2, or interpret
the relay and enable codes as a group acknowledgement. There is no known
status request to force a 22F sample. Stored target blocks use H even for
DHW. A confirmed optional second table (primary block + 1) supplies separate
wake and sleep words; an absent table is not two zero targets.

For an explicit 13E read or write, the gateway requires evidence of a
setpoint controller and reads application setup (0026) from that device.
Application codes 0 and 3 select 0028; codes 1, 2, 4 and 5 select
002A (hot tub), 002C (DHW), 002E (floor) and 0030 (pool) respectively.
Unknown or unsupported applications return FFFF rather than guessing.
These blocks have four little-endian words: occupied, unoccupied, away
and maximum. Setbacks 0/2/4, 1/3/5 and 6 select the first three words
respectively. With setbacks disabled, update all supported target words
but never the maximum. The six-byte floor layout without a maximum is
also supported when its support mask agrees. Do not write an extended
or ambiguous layout or half a word. After a change, return the target
actually read back, not the value in the host request.

Normal mode accepts one exclusive setting operation per physical port. Apart
from ModeSetting reads as described above, a supported setting Request
sends an addressed 2D7 and waits for a matching 2E7 from the same
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
Otherwise it sends a native 2DF Update, then requests the block again.
Response:Update contains the value actually read back, including when the
device kept the old value. A link ACK is not application confirmation.
If the update loses arbitration it has not been delivered: reread the
device, recheck its application or sensor setup where applicable, and
rebuild the update with the latest sibling values. Changing the underlying
target ends the transaction; contention does not restart its deadline.
A missed ACK, echo mismatch, bad end or missed timing has uncertain delivery.
Only readback follows, never a second write. There is no atomic lock
against another bus writer.

An unsupported field, incomplete write layout, lost transaction or expiry
returns FF for the value, not the requested setting. After a failed update
the on-device state may be unknown. Read it before deciding whether to try
again. The transaction budget is ten seconds; allow more than that on the
host to receive a negative reply from a quiet device. An overlapping setting
command requiring the same port returns BridgeError (Busy). An unsupported
value can be returned immediately without reserving the port. Raw mode is unaffected.
