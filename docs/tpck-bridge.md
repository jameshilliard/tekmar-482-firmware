# TPCK gateway protocol

Both firmware images start in Normal mode. At 9600 baud, 8N1, a host
can send a complete ordinary TPCK frame without a training command.
Normal mode implements gateway identification, device inventory, device
identity and thermostat telemetry. See [Normal method coverage](normal-mode.md) for supported services
and remaining gaps. Select Raw mode to exchange native frames. The NXP ROM
ISP uses its own handshake after an explicit handoff.

## Baud acquisition

A cold gateway listens at 9600 baud. At that speed send the first
request directly. For a different fixed baud, send two bursts of TPCK SOF bytes (`CA`), followed
by one isolated SOF. Each burst lasts about 8 ms (at least one byte).
Start each of the three writes 80 ms apart, then leave another 80 ms
before the first complete request. The bursts arm UART0. On the isolated SOF,
hardware measures the first low pulse of SOF (two bit periods). The
firmware corrects the UART divider and locks the rate upon reception of
a checksum-valid packet. Without a valid packet the tentative divider
reverts to 9600. Once locked, keep the serial rate fixed until reboot.
A TCP serial bridge must already be configured for the selected rate.
On a 12 MHz LPC2146 board, a verified SRAM image answered an ordinary
first request at 9600 and independently acquired 187500 from a cold
start, streamed native traffic and reentered ROM ISP.

## Envelope

The unescaped interior is one length byte (payload only), one packet type,
that many payload bytes, and the low eight bits of their sum including
length and type. Prefix every interior CA, 35 or 2F byte with 2F, including
length and checksum. Wrap the result in CA and 35. An unescaped CA restarts
the decoder. Dispatch only on a valid checksum followed immediately by an
unescaped 35. An invalid frame never executes a command or receives a reply.

Frames may follow each other with no inter-frame delay. In particular, a host
can submit one Send on each of several different ports without waiting for the
first port's TxResult. The receiver drains available UART FIFO bytes before
returning to line timing, dispatching at most one complete command per pass.

Type 06 is tRPC: service u8, method u32 little endian, remaining body.
Request (01) and Update (00) receive ResponseRequest (04) and ResponseUpdate
(03), respectively. An unknown method receives method zero on the matching
response service. Do not respond to a report or response.

The two numeric version methods are standard tHA and available in Normal
mode only. The 544BFxxx methods are **custom** allocations. Unless indicated,
the request body is empty. All integers are little endian.

| Method | Service | Response body |
| --- | --- | --- |
| 00000187 FirmwareRevision | Request, zero or two placeholder bytes | u16 numeric revision, 0 for this unnumbered custom image |
| 0000018F ProtocolVersion | Request, zero or two placeholder bytes | u16 tHA protocol version, currently 3 |
| 544BF001 BridgeCapabilities | Request | Three u8 values: version (2), mode (0 Normal, 1 Raw), port count (5). Remaining bytes are one little-endian feature bitmap: bit 0 Raw, 1 ROM update, 2 Normal trace, 3 error reporting, 4 Normal identity read, 5 traffic counters, 7 Normal status read, 9 Normal announcements, 10 Normal bitmap block reads and guarded edits. Bits 6 and 8 are retired. |
| 544BF002 SetBridgeMode | Update, mode u8 | Echo mode: 0 Normal, 1 Raw. Busy during an inventory walk, a started transmission, an outstanding host send or a local ACK. Switching modes clears the Normal inventory; returning to Normal also clears the ACK mask. |
| 544BF003 PortDrops | Request, port u8 | Port u8, valid records dropped u64, invalid records dropped u64. |
| 544BF004 FirmwareBuild | Request | Build revision without a terminator: 40 lowercase hexadecimal digits or `unversioned` |
| 544BF005 EnterFirmwareUpdate | Update | Empty positive reply. Finish transmitting it, drain the last UART stop bit, release peripherals and enter ROM ISP. No erase or application programming is performed by this call. |
| 544BF006 BridgeError | Response only | Rejected method u32, reason u8: 1 busy, 2 invalid service/body, 3 unavailable in this mode |
| 544BF007 Panel | Request | Boot and current inputs (two u32); Test presses and DIP transitions (two u64); LED mask, pin readback mask and lamp test active (three u32). |
| 544BF008 LedTest | Update | Empty. Starts the panel lamp sweep. |
| 544BF009 LinkAckMask | Update, mask u8 in Raw mode | Echo mask (bit N for port N). Zero disables local ACK. |
| 544BF00A CaptureCounters | Request | Two timer readings in microseconds (u32), five edge counts (u64), five last capture ticks (u32); both port arrays in physical port order. |
| 544BF00B NormalTrace | Request, empty; Update, port mask u8 | Normal only. Request returns the mask and five cumulative u64 trace-drop counts in port order. Update replaces the mask and echoes it; zero stops tracing. |
| 544BF00C ErrorReporting | Request, Update, Report | Both modes. Read/replace the source mask, read one error counter, or receive changed counters (below). |
| 544BF00D NormalIdentity | Request, port/subnet/node | Queues a read of identity block 00 in Normal mode. The native reply, not the gateway acknowledgement, identifies the device. |
| 544BF00E TrafficCounts | Request | Read gateway-owned traffic totals or enumerate native paths, in either mode. See below. |
| 544BF00F NormalBlockRead | Request, Report | Read a block on one physical port and receive each selected peer's native frame via the control channel. Normal only. |
| 544BF010 NormalStatusRead | Request, Report | Solicit native status, including multipart replies, without changing mode or trace configuration. |
| 544BF011 NormalBlockEdit | Update, Report | Compare an addressed block with the host's baseline and send at most one native block update. Does not select Raw mode. |
| 544BF012 NormalAnnounce | Update, Report | Send one known native announcement on one physical port; report the transmit outcome. |

Version 2 event counts start at zero at boot, survive mode changes and
saturate at 2^64-1 instead of wrapping. Subtract snapshots from the same
boot. Hardware timer readings and per-frame timestamps are still u32 and
wrap; their width does not limit lifetime counts. The host requires the
current custom wire version and does not decode previous diagnostic
layouts. This check does not apply to commands supported by stock firmware.

An ordinary capability probe must never change operating mode. Stock
NullMethod means the extension isn't available; a timeout is not evidence
of stock firmware. A client should issue the update only after positive
discovery, then synchronize with the ROM and verify the LPC2146 part ID
before programming. Never retry an ambiguous handoff or flash write.

Byte-exact frames (spaces separate fields, not transmitted bytes):

```text
ProtocolVersion request:   CA 07 06 01 8F010000 0000 9E 35
ProtocolVersion response:  CA 07 06 04 8F010000 0300 A4 35
BridgeCapabilities:        CA 05 06 01 01F04B54 9C 35
Normal capabilities:       CA 0A 06 04 01F04B54 020005BF06 70 35
EnterFirmwareUpdate:       CA 05 06 00 05F04B54 9F 35
Drained before handoff:    CA 05 06 03 05F04B54 A2 35
```

## Addressed Normal reads

With capability bit 10, send Request `544BF00F` with `port:u8 subnet:u8
node_mask:u32 block:u16 selector_mask:u8 selector:u8`. Select nodes 1..31
with bits 0..30. An empty bitmap or set padding bit 31 is invalid. For a plain block both
selector bytes are zero. For an indexed block the selector must fit its
mask: for example, `07 03` selects schedule 3 ignoring the other flags in
its first data byte. The gateway sends only a class-1, source-zero
`2D7`, with the entire bitmap and a little-endian u32 selector argument.
The request cannot send an update or change trace, mode or reporting.

An immediate Response:Request repeats the ten request bytes if admitted.
One read fits on each port; an overlap gets BridgeError Busy and no terminal
event of its own. A busy port or silent peer does not block the host's other
tRPC commands. Queued settings take priority over reads; admission does not
guarantee the line becomes available before the ten-second gateway deadline.

Each distinct matching node produces one Report on the same method, via the
control FIFO independently of trace and report subscriptions. Its body is
`port:u8 status:u8 tx:u8 gateway_ms:u32 native_header_and_payload`.
Status 4 means more nodes remain; 0 includes the last frame and completes
the read. All parts carry the same actual transmit outcome. The gateway waits
up to ten seconds to acquire the line, then ten seconds for each next distinct
peer. Duplicates, including a change to the header's node flag, do not extend it.
A failed terminal has exactly three bytes, `port:u8 status:u8 tx:u8`:
1 expired (earlier parts still valid), 2 TX pin unavailable, 3 link fault,
5 control FIFO full (evidence lost, not a silent peer). Every admitted
request has exactly one terminal. No per-node completion packet is needed.
No response packet or temperature is synthesized. An ACK or
missing ACK by itself is not decisive: after transmission the receiver waits
for a valid on-time `2E7` from that subnet/node, with the requested block,
an intelligible support mask and, if indexed, the supported selector.
Header flags, partial supported data and extension bytes are returned intact.
This is fresh peer evidence, not proof the read caused the announcement.

No host token is needed. A port's terminal report is enqueued before a
new read on that port can be accepted. Arm the waiter at its own admission
in UART order, not when a host coroutine resumes. After a lost or cancelled
admission, an inline capabilities request fences earlier acceptance replies;
it does not cancel an accepted read. Raw mode and ROM handoff cannot abandon
an outstanding read. Timeouts never cause a retry after a completed frame.

## Guarded block edits

When feature bit 10 is present, send **Update** `544BF011` with
`port:u8 subnet:u8 node:u8 block:u16 selector_mask:u8 selector:u8`,
always exactly one node (1..31), followed by `support:u8 old_len:u8
old_fields[old_len] new_fields[remaining]`. Fields are positional. Use the
support mask and all old bytes from the addressed peer, and a documented
[block writer](tn4-blocks.md). The gateway rejects read-only blocks, wrong
lengths or selectors, changes to unsupported bytes, and no-op proposals.
Block 0F can extend five old bytes with its prescribed zero; 2E can have
three or four words. 5A permits only a runtime reset to zero.

Response:Update echoes the request on admission. The port shares its slot
and deadline with Normal reads. The gateway sends an addressed `2D7`,
waits for a fresh matching `2E7`, and compares the support mask and **every
baseline byte**, including siblings the user did not edit. A mismatch, even
one observed while waiting to acquire the line for the update, retires the
transaction without writing. An exact match permits one class-1, source-zero
`2DF` addressed to that node. Unlike a read, a write that loses arbitration
is **not** retried. Once the write is driving the line, its actual completion
is reported even if the admission deadline elapses. This is not a lock on the
remote device: a physical edit can still race the final transmission.

The one terminal Report is `port:u8 end:u8 guard_tx:u8 update_tx:u8`.
End 0 means update attempted (not applied), 1 baseline changed, 2 expired
before the update, 3 guard transmission failed, 4 update TX pin unavailable.
TX: 00 ACK, 01 no ACK, 02 arbitration lost, 03 echo mismatch, 04 bad end,
05 missed timing, FE pin unavailable, FF no completed attempt. For ends 1,
2 and 3, update_tx is FF. Admission, a link ACK, and the guard's matching
report **are not write confirmation**. After a possibly delivered update,
start a separate NormalBlockRead and compare the edited fields. Do not
automatically resubmit an edit after a host timeout.

## Normal announcements

With feature bit 9, **Update** `544BF012` takes `port:u8 id:u16le data...`.
There is no all-port selector. The gateway builds the header, source 0/0:

| Native ID | Data | Priority |
| --- | --- | --- |
| 010 ZoneSynchAdjustment | Cycle 0/1, purge 0/1, target 0..2. | 0 |
| 21F SetpointRemoteEnable | Group 1..12, enable 0/1. This is not addressed to a single appliance. | 0 |
| 2FF SceneReport | Scene 0..7 and a little-endian duration in minutes. | 0 |
| 31F RealTimeClockReport | Five [standard-time fields](gateway-traffic.md#clock-representation), optionally followed by the DST-mode byte. | 0 |
| 3DF NetworkInitRequest | Empty. No inventory or other application reply implied. | 1 |

Response:Update echoes the complete admitted request. A competing extension
operation on that port is rejected as Busy; it cannot replace the announcement.
Other ports continue independently. No receive observation completes the send.
Exactly one terminal Report `port:u8 tx:u8` follows admission: 00 link ACK,
01 no ACK, 02 arbitration lost, 03 echo mismatch, 04 bad end, 05 missed
timing, FE no TX pin, FF expired **before** starting. A send can wait ten
seconds for the line. An attempt that has started finishes; even arbitration
loss is not retried. A host timeout has unknown outcome and is not equivalent
to FF.

These sends use the Normal link driver, counters and optional broadcast TX
trace. They do not change modes or reporting and never manufacture a group,
zone or thermostat state report. The stock tHA group and DateTime methods
retain their distinct all-port behavior.

## Normal status reads

Request `544BF010` takes `port:u8 filter:u8 subnet:u8 node:u8 id:u16 args...`.
Filter 0 requires zero subnet/node and follows the first matching peer; filter
1 selects the specified native subnet and five-bit node, including node zero.
The filter selects the **reporter**, not the I/O payload address or snowmelt
zone number. Admission echoes the complete request. Block and status reads
share one slot per physical port. Status reads have ten seconds from admission. Background
probes on that port wait; normal settings, other ports and host requests keep
running. No trace subscription is required.

The gateway sends a class-zero, source-zero read once it wins arbitration.
Only the following request IDs and argument shapes are admitted (IDs in hex):

| Request / report | Arguments | Matching evidence |
| --- | --- | --- |
| 011 / 010, 307 / 30F, 317 / 31F, 327 / 32F, 3F7 / 3FF | empty | Zone sync, warm weather shutdown, clock, outdoor, or master report, respectively. A header-only MasterPresent establishes presence but not topology. |
| 200 / 200 | 01 | Access status with the request bit clear; never echoes or changes access. |
| 322 / 323 | subnet, u32 RH selector | Exact reporter on the selected subnet; all declared sensor bytes present. |
| 1F4 / 1F3 | I/O subnet, I/O node, channel | All three payload bytes echoed. |
| 1F6 / 1F5 | I/O subnet, I/O node | Distinct channel IDs from one source; changing counts do not establish completion. |
| 17F / 17E | role/number byte, key count, 1..61 ordered keys | Matching role and one-based reporting number; all distinct requested keys, potentially over several reports. |

Every accepted query ends with exactly one terminal Report. Before that, each
matching peer frame is a Report with `port 00 gateway_ms:u32 native_frame...`.
The header, lock bit, optional bytes and extensions are unchanged. The terminal
body is `port end tx`: end 1 complete, 2 deadline (retain partial data), 3
control FIFO full (result is unreliable), 4 transmit fault. TX is 00 link ACK,
01 no ACK, 03 echo mismatch, 04 bad end, 05 missed timing, FE no TX pin or FF
no completed attempt. A link ACK alone never completes a query. On FIFO loss
the gateway increments the port's 64-bit native-read-drop counter and reserves
the terminal until the FIFO can accept it; it never silently declares success.

## Traffic counters

Counters run from boot without a host, a subscription, or a trace mask. They
survive reconnects and mode switches, but not a reset. All counts are
saturating little-endian u64. `counts` below is four such values in order:
valid peer RX, gateway TX with link ACK, gateway TX completed without ACK,
and other failed TX attempts. An arbitration retry counts as another attempt.
TX ACK does not mean the device applied a command.

| Request body | Response body (little endian) |
| --- | --- |
| Empty | Directory capacity and used entries (two u16). |
| Port (u8) | Same port, counts, invalid peer RX, valid RX without a directory slot, TX without an attributed slot (three more u64). |
| Index (u16) | Same index, path (port/subnet/node, three u8), counts. Indices are stable from zero up to but not including used. |
| Path (three u8) | Same path, present (u8, 0 or 1), counts. An unknown path returns zeros without creating an entry. |

An RX path is the valid native header's source with the low five node bits.
Locally transmitted echoes are not received peer frames. Invalid frames are
counted only per bus. A TX path is the destination of a block update or the
sole selected node in a block request, stage heat demand request, purge or
stage override. The stage operations use a 24-node mask, block requests a
31-node mask with one padding bit. Broadcasts, multi-target requests and
messages without a verified native node destination remain in the port totals.
Unsent requests do not count. Up to 256 distinct paths are retained without
eviction; if the directory fills, bus totals and previously registered paths
continue counting.

## Raw records

TPCK type 01 carries the following payloads. The first two bytes are
always service and physical port (0..4). Multibyte metadata is little
endian. A native body is the four-byte big-endian tN4 header followed
by the payload length specified in its low six bits.

| Service | Direction | Fields after port |
| --- | --- | --- |
| 00 Rx | Gateway to host | Gateway uptime in ms (u32, wraps), native body. Only link-valid frames; both acknowledged and unacknowledged frames are forwarded. |
| 01 Send | Host to gateway | Native body including source. Gateway supplies the wire CRC, stuffing and trailer. |
| 02 TxResult | Gateway to host | Status 0: sent and link-acknowledged, no extra byte. Status 1: rejected before send, one reason byte (1 invalid body, 2 pin unavailable, 3 wrong mode, 4 local ACK active). Status 2: attempted, one reason byte (1 no ACK, 2 arbitration lost, 3 echo mismatch, 4 bad end, 5 missed timing). |
| 03 PortError | Gateway to host | Reason u8: 1 send in progress on this port, 2 invalid port. This is not the terminal result of the outstanding send. |
| 04 RxInvalid | Gateway to host | Gateway uptime in ms u32, reason u8 (1 stuffing, 2 CRC, 3 CRC delimiter, 4 bad end), bit count u16, captured destuffed bits starting with SOF, MSB first, zero pad in last byte. Not a device update. |

There is at most one outstanding send per port. Wait for its terminal
TxResult before submitting another on the same port. Other ports can
progress independently. There are no implicit retries, replacements,
flushes or send deadlines. A host read timeout does not establish whether
a frame was sent. Even a link ACK needs an application-level report or
readback to confirm that a setting changed.

On the same port, a peer frame received while the send is still waiting
for the bus appears before TxResult. It remains valid capture data, but
cannot answer that request. For a completed transmission the gateway
queues TxResult before a subsequent peer reply. Arm the application
collector at that wire record (ACK or no ACK), not when the sending
coroutine gets CPU time. A single serial read may contain both records.
This orders observations; the native bus does not tag a solicited reply.

## Normal-mode trace

This is a tap on Normal traffic, not a third operating mode. The gateway
continues discovery, ACKs, reads, writes, retries and tRPC reporting.
Enabling trace does not change reporting, reset inventory or permit raw
Send. It is off at boot and on entry to Raw. A serial disconnect is not
detectable; an independent host can always clear the trace mask in Normal.

TPCK type D0 is gateway-to-host only. Its common prefix is event u8, port
u8, gateway uptime in ms u32 (little endian, wraps). After the prefix:

| Event | Data | Meaning |
| --- | --- | --- |
| 00 Rx | flags u8, complete native body | Peer frame; bit 0 means link ACK observed, bit 1 early start (not passed to the Normal inventory). Gateway TX echo is excluded. |
| 01 Tx | origin u8, outcome u8, intended native body | One gateway attempt at completion, including an arbitration loss before a retry. Origins: 0 setting transaction, 1 broadcast, 2 line discovery, 3 native read (background or explicit). Outcome 0 link ACK, 1 no ACK, 2 arbitration lost, 3 echo mismatch, 4 bad end, 5 missed timing. A failed attempt need not have put the complete body on the bus. |
| 04 RxInvalid | reason u8, bit count u16, captured bits | Same reason and bit packing as Raw RxInvalid. Not a valid peer update. |

Trace records preserve observation order with respect to each other.
Host replies and reports may overtake unstarted trace records; use the
gateway timestamps for native timing, not UART arrival time. Trace mask
replies delimit the capture: queued older records precede the reply.
Trace has three dedicated slots; a full queue drops new trace records,
increments the affected port's lifetime drop counter and leaves Normal
operations running. Counters do not include Raw-mode drops. Restrict the
port mask or use a higher negotiated baud on busy installations.

## Error reporting

Independent of stock reporting, Normal trace, Raw capture and the ACK mask.
Off at boot; switching between Normal and Raw leaves the selection intact.
There is no subscription in ROM ISP. A disconnected host cannot clear its
selection; a subsequent host can read or replace it without switching modes.

- Request with no body: returns the one-byte source mask. Update with one
  byte: replace and echo the mask. Bits 0..4 select physical ports; bit 5
  selects the gateway itself. Zero stops reports; reserved bits are rejected.
- Request with two bytes, source and reason: returns that counter, even
  when disabled. An invalid source/reason combination is rejected.
- Response to a counter read and unsolicited Report (service 02) use
  identical bodies: source u8, reason u8, last observation uptime in ms u32,
  lifetime count u64. A zero count means never observed. The timestamp wraps.

| Source | Reason codes (hex) |
| --- | --- |
| Port 0..4: receive | 01 stuffing, 02 CRC, 03 CRC delimiter, 04 bad end, 05 valid frame without link ACK (can include own echo), 06 early start, 07 onset of stuck dominant, 08 local link response drive fault |
| Port 0..4: transmit | 09 no ACK, 0A arbitration lost, 0B echo mismatch, 0C bad end, 0D missed timing, 0E pin unavailable |
| Port 0..4: output loss | 0F Raw valid record dropped, 10 Raw invalid record dropped, 11 Normal trace dropped, 12 Normal block or status part dropped. The first three match PortDrops and NormalTrace. |
| Gateway (5) | 80 UART overrun, 81 parity, 82 framing, 83 break, 84 TPCK checksum, 85 TPCK framing, 86 new SOF interrupting a partial packet, 87 pending address-change notice evicted |

Counters start at boot, not at subscription. Enabling does not replay old
events. While enabled, repeats of a source/reason can share a single report
with the latest timestamp and total. Change the mask to zero to stop the
stream; already queued reports precede the acknowledgement. This is not a
frame log: diagnostics yield to normal traffic. Use trace or Raw capture
for frame bytes. Neither no ACK nor arbitration loss alone proves a fault.
TX totals count physical attempts, not host commands. UART counters count
hardware flag assertions, not the number of missing bytes (see the UART0
line status register in [NXP UM10139](https://www.nxp.com/docs/en/user-guide/UM10139.pdf)).
