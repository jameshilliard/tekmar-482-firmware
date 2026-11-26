# Gateway-originated native traffic

A registered message layout does not mean the 483 gateway transmits that
message, nor that every product responds to it. The following operations
have send-side evidence in 483 SW 11. Message IDs below are hexadecimal.

| Operation | Request or announcement | Evidence and expected feedback |
| --- | --- | --- |
| Discover / read block | `2D7` | `MessageQueue.request_block` and `DevicePoller`: exact subnet, node bitmap and a 32-bit argument. A matching `2E7` is data; an ACK alone is not. |
| Update block | `2DF` | `MessageQueue.update_block`: destination node and the block-specific **write** layout, not a copy of a masked report. Read the block again to check supported fields. See [addressed blocks](tn4-blocks.md). |
| Remote enable, group or device UI | `21F` | Both `setpoint_remote_enable_by_group` and `setpoint_remote_enable_by_dev` send **group ID, enable (0/1)**. Neither contains a device address. |
| Select scene | `2FF` | `push_scene_change`: scene code and duration. Device responses come from their own settings; a received announcement is not acceptance. |
| Request water/control status | `3F7` | `poll_water_temperatures` sends MasterQuery; MasterPresent (`3FF`) identifies the responding control and its status, not the end of all subsequent telemetry. |
| Query / adjust zone synchronization | `011` / `010` | `zone_synchronization_query` and the test-settings model. The adjustment has three bytes: cycle selector 0/1, purge 0/1, target 0 outdoor reset / 1 reset only / 2 setpoint. The models disagree about the cycle flag's polarity, so keep the wire selector explicit. |
| Announce date and time | `31F` | `rtc_report` and `RealTimeClockReportIn.write` produce the five-byte form described below. `317` is the separate request for a clock report. |
| Query gateway access status | `200` | `query_lock_state` sends byte `01`. A status uses the same ID: bit 0 clear, bit 1 access-open. Do not mistake a request echo for a status. This query does not change access settings. |
| Query I/O | `1F6` / `1F4` | The I/O channel model requests discovery by subnet/node or one channel by subnet/node/channel. Match the fields in replies `1F5` / `1F3`. |
| Query snowmelt | `17F` | The snowmelt model chooses keys for its configured application, zone or sensor. Match the reported address and individual keys in `17E`, not just the source port. |

The 483 message serializer sets native header priority 0 on its normal
outgoing messages. Its serial all-ports selector is **not** a tN4 address.
In 482 Raw mode the host sends the same native body separately on each
selected physical port. A busy line, missing ACK or UART deadline on one
line is not a result for another. Normal-mode stock-style broadcasts
retain their own priority and port behavior.

## Setpoint groups

Block `26` of an identified 161 or 162 contains application mode in
logical byte 1 and enable group in byte 7. The mask tells which fields
exist. SW 11's setpoint-control model uses remote enable for applications
0 (setpoint on/off), 1 (hot tub), 3 (setpoint PWM) and 4 (floor), not 2
(DHW) or 5 (pool). Zero means no assigned group. The 482 group controls
use IDs 1 through 12. Do not manufacture a group from the node address,
send to a different group when the setting is absent, or imply that other
members of the same group are unaffected.

`20F` reports group ID and flags: bit 0 requested and bit 4 actual. Both
belong to the reporter on that physical line. They can disagree. Retain
the reporting source and any other flag bits. Neither receipt of one's own
`21F` nor a link ACK confirms a change; silence is not an off report.

## Clock representation

The SW 11 transmit and receive paths use standard time plus a one-hour
daylight adjustment. To send a local daylight time, subtract one hour
**before** computing weekday, day, month and year, then set bit 4 of the
month byte. Thus a Monday 00:30 daylight reading can carry a Sunday
23:30 timestamp. This is not UTC. No timezone offset or region is present.
The optional sixth byte in the alternate layout is a DST **mode**, not a
second adjustment; retain its value and any future extension bytes.

When the entire month/flags byte is `FF`, the sender does not provide a
calendar date or a meaningful DST flag. Preserve hour/minute and weekday;
do not fill in today's date. Preserve invalid fields or a contradictory
weekday for diagnosis without publishing a civil timestamp. Stock 482
tHA DateTime, which does not carry a DST flag, remains a separate
[Normal-mode update](normal-mode.md#clock-updates).
