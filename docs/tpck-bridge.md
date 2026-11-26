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
unescaped 35. An invalid frame has no reply or side effects.

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
| 544BF001 BridgeCapabilities | Request | Four u8 values: extension version (1), current mode (0 Normal, 1 Raw), port count (5), feature flags (bit 0 bidirectional Raw, bit 1 ROM update, bit 2 Normal trace) |
| 544BF002 SetBridgeMode | Update, mode u8 | Echo mode: 0 Normal, 1 Raw. Busy during an inventory walk, a started transmission, an outstanding host send or a local ACK. Switching modes clears the Normal inventory; returning to Normal also clears the ACK mask. |
| 544BF003 PortDrops | Request, port u8 | Port u8, valid records dropped u32, invalid records dropped u32. |
| 544BF004 FirmwareBuild | Request | Build revision without a terminator: 40 lowercase hexadecimal digits or `unversioned` |
| 544BF005 EnterFirmwareUpdate | Update | Empty positive reply. Finish transmitting it, drain the last UART stop bit, release peripherals and enter ROM ISP. No erase or application programming is performed by this call. |
| 544BF006 BridgeError | Response only | Rejected method u32, reason u8: 1 busy, 2 invalid service/body, 3 unavailable in this mode |
| 544BF007 Panel | Request | Seven u32: boot inputs, current inputs, Test press count, DIP transition count, LED mask, pin readback mask, lamp test active. |
| 544BF008 LedTest | Update | Empty. Starts the panel lamp sweep. |
| 544BF009 LinkAckMask | Update, mask u8 in Raw mode | Echo mask (bit N for port N). Zero disables local ACK. |
| 544BF00A CaptureCounters | Request | Twelve u32: two timer readings in microseconds, five edge counters and five last capture ticks in port order. |
| 544BF00B NormalTrace | Request, empty; Update, port mask u8 | Normal only. Request returns the mask and five little-endian u32 cumulative trace-drop counts in port order. Update replaces the mask and echoes it; zero stops tracing. |

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
Normal, Raw/update/trace:  CA 09 06 04 01F04B54 01000507 B0 35
EnterFirmwareUpdate:       CA 05 06 00 05F04B54 9F 35
Drained before handoff:    CA 05 06 03 05F04B54 A2 35
```

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
| 01 Tx | origin u8, outcome u8, intended native body | One gateway attempt at completion, including an arbitration loss before a retry. Origins: 0 setting transaction, 1 broadcast, 2 line discovery, 3 background block read. Outcome 0 link ACK, 1 no ACK, 2 arbitration lost, 3 echo mismatch, 4 bad end, 5 missed timing. A failed attempt need not have put the complete body on the bus. |
| 04 RxInvalid | reason u8, bit count u16, captured bits | Same reason and bit packing as Raw RxInvalid. Not a valid peer update. |

Trace records preserve observation order with respect to each other.
Host replies and reports may overtake unstarted trace records; use the
gateway timestamps for native timing, not UART arrival time. Trace mask
replies delimit the capture: queued older records precede the reply.
Trace has three dedicated slots; a full queue drops new trace records,
increments the affected port's counter modulo 2^32 and leaves Normal
operations running. Counters do not include Raw-mode drops. Restrict the
port mask or use a higher negotiated baud on busy installations.
