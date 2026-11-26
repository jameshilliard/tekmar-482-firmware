# Bridge interface bring-up

The LPC2146 has a UART0 host link and five tN4 receive paths. The table
uses the logical port numbers in the stock J1196J sampling routines, not
thermostat addresses. Only explicit TX and receive-ACK commands can drive a bus.

| Port | RX pin | Capture input | TX pin | Stock sample routine |
| ---: | --- | --- | --- | --- |
| 0 | P0.4 | CAP0.1 | P0.28 | 0x7f24 |
| 1 | P0.11 | CAP1.1 | P0.13 | 0x823c |
| 2 | P0.6 | CAP0.2 | P0.22 | 0x8554 |
| 3 | P0.10 | CAP1.0 | P0.8 | 0x8868 |
| 4 | P0.2 | CAP0.0 | P0.29 | 0x8b84 |

The pin functions and timer capture registers are documented in
[NXP UM10139, chapters 6 and 15](https://www.nxp.com/docs/en/user-guide/UM10139.pdf).
UART0 uses P0.0 (TXD0) and P0.1 (RXD0) independently of these pins.

## Passive probe

With this build running, use `uv run --script tools/board.py probe "$PORT"
--baud 187500` (on one line). For an SRAM trial, the existing
`tools/ram_trial.py PORT RAM.bin` helper accepts `--probe` and expects a
synchronized ROM session at 9600 as usual. No flash write is required.

After the normal `A\n` training prefix, `RX-BEGIN\n` resets both timers
to a 1 MHz time base, selects capture on both edges, and replies `OK RX`.
Send `RX-END\n` with its own prefix to stop the window. The host helper
waits one second. The reply is `RX t0 t1 seen p0 p1 p2 p3 p4`, with fields
in hexadecimal. The first two fields are elapsed microseconds on each
timer. Bit N in `seen` indicates a capture on port N; its timestamp is the
last edge in microseconds on that port's timer. No capture means the
timestamp field is ignored. Keep manual windows below the 32-bit timer wrap
(about 71 minutes).

Starting again discards the previous window. Ending the window or entering
ISP stops both timers, clears their capture flags, and restores the pin
multiplexer and timer power bits. No GPIO drive, external match output, or
CPU interrupt is enabled. Hardware latches the last edge without counting
intervening edges; this is not a packet decoder or logic analyzer.

In the SRAM bring-up, both timers advanced by about one million ticks per
host second. In a ten-second window, ports 0, 1, and 2 captured edges; ports
3 and 4 did not. A quiet input is not a failed input. To validate those
paths individually, repeat with a known active signal on each port.

## Stream decoded frames

To repeat a trial from the installed **development** image, without a flash
write or a button press:

```sh
make ram-trial
uv run --script tools/ram_trial.py "$PORT" \
  target/ram-trial/armv4t-none-eabi/release/tekmar-482-firmware.bin \
  --from-app --frames 60
```

This verifies the image in SRAM, listens on all five ports, then restarts the
installed dev image and verifies its identity. When the receiver build is
already running, use `uv run --script tools/board.py frames "$PORT" --baud
187500 --seconds 60` instead (one line).

The input bit period measured on the bus was about 2500 us (400 bits/s).
The receiver alternates rising/falling timer captures and samples at bit
centers, re-centering on each transition. It synchronizes after seven low
bits. SOF is high. Header, payload and 15-bit check word are MSB-first.
Stuffing continues across those fields; the check is computed over SOF and
the unstuffed header and payload (polynomial 0x4599). The low CRC delimiter,
ACK slot, low ACK delimiter and seven low EOF bits are checked separately.
Passive capture never asserts an ACK, sends a probe, or drives any bus.

Send `A\nRX-STREAM\n`; the gateway replies `OK FRAMES\r\n`. Each record is
one ASCII line, `F port start_us crc status header_and_payload\r\n`. The
timestamp is the SOF edge on that port's microsecond timer (32 bits, wraps
at about 71 minutes; the two timers start sequentially). The check word
and bytes are hexadecimal. The host independently checks length and CRC,
then prints the priority, message ID, subnet, node and raw payload using the
483 receiver's header layout. Recognized IDs also get decoded payload fields. An ACK is
link-layer only, not confirmation that an application acted on a message.

| Status | What the receiver observed after a matching CRC |
| --- | --- |
| A | ACK and valid end |
| N | No ACK, valid end |
| D | Invalid CRC delimiter; ACK not determined |
| E | ACK followed by an invalid ACK delimiter or EOF |
| M | No ACK followed by an invalid ACK delimiter or EOF |
| L | ACK sampled and valid end; this gateway drove the ACK slot |
| X | ACK sampled but bad end; this gateway drove the ACK slot |

Bad stuffing, truncated data and CRC mismatches do not produce frame
records. A CRC-matching body with a damaged trailer is diagnostic, not a
successful exchange. Copies and retries on the same or different ports
remain separate. Send a single LF without the training prefix to stop;
the receiver drains the UART queue and replies `END nnnnnnnn\r\n`.
The hex count is complete records dropped due to UART backpressure; it is
not a count of missed electrical edges or rejected candidates.

In a 70-second SRAM trial, the host verified 74 records: 26 acknowledged on
port 1, 44 acknowledged on port 2, and four CRC-matching bodies on port 0
with no ACK and a bad end. No UART queue drops occurred. Ports 3 and 4
remained quiet; they have not been validated with an active source.

### Application payloads

The host decodes these native IDs without changing the firmware or sending bus
traffic. Field layouts come from the 483 application (`message_types_in.rb`,
`message_types.rb`, `subnet_in.rb` and `snow_pair_in.rb`), not from tRPC
method IDs. Link trailer status is always reported separately.

| ID | Decoded fields |
| --- | --- |
| 0x17e | Snowmelt source and ordered key/value pairs: command status, water detection, heat sequence, state/substate, weather cutoffs, zone and group IDs, sensor flags, slab/edge readings, outputs, contacts, supply/return fields, keep-alive and melting time. |
| 0x1f2 | I/O report subnet, node, channel, type and 16-bit value |
| 0x22f | Setpoint enable, setback, operating/demand/actuator/relay states, priorities, three temperatures and status excuses |
| 0x23f | DHW enable, setback, priorities, operating/demand/actuator states, target and tank temperatures |
| 0x24f | Plant device ID/count/type; subtype 6 (modulating boiler) state, pump, modulation levels, inlet/outlet raw values, output and return protection |
| 0x2d7, 0x2df, 0x2e7 | Block request (target subnet and node mask), update (address and values), report (support bitmap and packed values). Setpoint blocks 5/6 decode Wake, Unoccupied, Occupied, Sleep and Away in degE. |
| 0x31f | Real-time clock report: time, weekday, zero-based date fields, year offset, DST flag and optional DST mode. This is a remote device report, not the gateway or host clock. |
| 0x32f | Outdoor temperature; the 482 sends a second copy of the word (J1196J 0x3dd8) and reads the first (0x3e38). Show both even if they differ. |
| 0x337, 0x33f | Maximum boiler percent; air group and requested cycle length |
| 0x367 | Water subnet, stage, override, priority, pump flags and delay; target temperature only when output is on |
| 0x36f | Air group, mode, demand, schedule, output states, local/remote temperatures and target |
| 0x3c7 | Relay heartbeat water subnet, split into bus and control |
| 0x3cf | Supply, return, target and optional DHW supply temperatures, heat/cool mode, loop/pump percentages, setback and limit flags |
| 0x3d7 | Network error code, instance and priority |
| 0x3df, 0x3f7 | Network init request and master query. Neither has a defined payload; extra bytes are retained. |

Known temperature encodings use the [tekmar degH and degE definitions](https://www.watts.com/our-story/brands/tekmar/tha).
The wire value is retained. Missing-temperature codes are not converted to
degrees (see J1196J 0x594c); zero is shown raw because zero-filled inactive
fields don't establish a measured temperature. Values without a confirmed
scale or enum keep their numeric codes.

Unknown snowmelt keys retain both bytes as a little-endian value. Unrecognized
trailing bytes (including byte 12 of observed 0x22f reports) appear as
`extension_hex`. A known payload too short for its layout is marked
`truncated`, not interpreted using missing bytes. Unknown message IDs still
display their header and raw payload.

To save an exact UART stream, add `--save-capture capture.frames` to
`board.py diagnostics`, `frames`, `send` or an SRAM trial with `--frames`.
This creates a new file (never overwrites one) and retains raw records,
including diagnostic rows and the final `END`, instead of terminal output.
Replay it using only the Python standard library:

```sh
python3 tools/frames.py capture.frames
```

Replay uses the same decoder as the live monitor. A bad UART record, missing
terminator or reported queue drop returns a nonzero exit status. A CRC-valid
tN4 body with a bad bus trailer is printed as a diagnostic as in live mode.

### Receiver diagnostics

`board.py diagnostics PORT --seconds 60` (or `ram_trial.py --frames 60
--diagnostics`) uses `RX-DIAG`. The wire stream is identical to `RX-STREAM`
except for the `OK DIAG` greeting and five final `D` rows before `END`.
Each row has a decimal port followed by thirteen eight-digit hex words:
captured edges, SOFs, stuffing failures, CRC mismatches, ACK with valid end,
no ACK with valid end, bad trailers, in-progress candidate at stop (0/1),
UART frame drops, maximum edge service delay in microseconds, and last
received and calculated mismatching CRCs (zero until a mismatch), and
the sampled physical input levels (bit 0 at capture start, bit 1 at stop;
1 = dominant). Archives with the earlier twelve-word rows still replay.
At startup the decoder samples the pin to choose its first edge. A port
that stays high with no captured edges cannot provide the idle interval
required for transmit; the firmware does not force a send on it.

The host independently checks each body CRC, balances all candidates against
their outcomes, and reconciles delivered frames and drops for all five ports.
Service delay measures from the capture latch to re-arming the next edge;
the timer has no FIFO or hardware overrun counter. A clean summary does not
prove a pulse was never missed.

### Acknowledging incoming frames

Use `board.py diagnostics PORT --ack-port N` or `ram_trial.py --frames 40
--ack-port N` to participate in the link ACK on **one** physical port.
All other inputs remain passive. This mode does not send any application
frame, retry any transfer, or change a device setting. It is useful on a
two-device line: a passive analyzer cannot acknowledge the other device.

The UART command `RX-ACK N` replies `OK ACK N` or `ERR ACK-PIN` if the
driver is unavailable. Like the stock J1196J routines at 0x470c, 0x4ad0
and 0x4b1c, the gateway requires a matching CRC and a low CRC delimiter
before asserting the following ACK slot. A missed start is not retried.
A stop byte finishes an already-started pulse and releases the driver.
`L` and `X` identify locally driven, sampled ACKs; another receiver can
drive the same slot. A local ACK is not evidence of a peer ACK.

Before the per-port summaries, `K port pulses late` reports two hex counts:
ACK pulses started and observed deadlines too late to drive. The pulse count
can exceed delivered local-ACK frames, e.g. if capture stops before EOF.
The boot default, background receiver and ordinary RX-DIAG do not drive ACK.

### One-shot transmit

`board.py send PORT --tx BUS HEX --seconds 10` sends one native header and
payload while monitoring all five ports. The equivalent SRAM trial takes
`--frames 10 --tx BUS HEX`. The host supplies the complete header, including
source, and the payload; the firmware supplies stuffing, bus CRC and trailer.
The UART request is `A\nTX BUS HEX CHECK\n`, where CHECK is four hex digits
of CRC-16/CCITT (poly 0x1021, initial 0xffff, no reflection or final XOR)
over one binary port byte followed by the header and payload. An invalid
length, port or check returns `ERR TX` without driving a pin.

After `OK TX`, the sender waits for 16 quiet bit periods, attempts the
frame once, checks its receive echo at each bit center and releases the
selected driver before emitting `T port result`. It does not retry.
`ACK` and `NOACK` describe only the link slot; `ARBITRATION` denotes a
dominant bit while sending recessive data, `ECHO` the reverse mismatch,
and `BAD-END` a dominant delimiter or EOF. `LATE` aborts a missed timing
window, `BUSY` means no idle opportunity within five seconds, `PIN-BUSY`
means the TX pin was not a free GPIO input, and `STOPPED` means the host
ended capture before completion. The host requires both an ACK result and
the matching CRC-verified echo. Verify device changes independently with
a fresh application-level readback. Capture continues until the host stops
it; diagnostic summaries and END are the same as for RX-DIAG.

Verified on a physical thermostat: an SRAM build requested heat block 5,
changed Away by one degE step, and restored it. Each update produced a
report; separate addressed requests confirmed both the changed value and
the complete original block after restoration. No flash write was needed.

To exercise the local driver and capture input on every port, use
`board.py echo-test PORT` (one port: `--bus N`) or `ram_trial.py --echo-test`.
This sends a block read request with an empty target-node bitmap, once on
each selected line. Success requires a matching body, CRC, physical port,
valid trailer and transmitter result. A peer ACK is reported separately
and is not required for a port with no peer. A local echo does not verify
field wiring or the receiver in an external device. Normal `send` still
requires an ACK.

### Front panel

The J1196J initializer copies the port index table (flash 0xc4a4) to
0x40000034. In port order the stock outputs at 0xc50c are P1.22, P1.21,
P1.18, P1.23 and P1.25; Test is P1.16. All six are active low. P1.24
(logical output 5) has a different polarity and no identified front-panel
function; the driver leaves it alone. Power and the RS-232 TX/RX lights
are not in this LED table; the manual describes them as power/serial
activity indicators. GPIO readback does not test those lamps optically.

`board.py panel PORT` reads the power-on and debounced Reporting DIP
(P0.7), Test button (P0.14), change counters and LED levels. Both inputs
are active low. A button press or `board.py led-test PORT` walks bus 0..4
then Test, 500 ms each. The host checks all six pin readbacks and that the
walk ends. The RAM trial accepts `--panel-test`. Outside the walk, a bus
LED denotes a CRC-matching frame seen in the last minute, including an
intact body with a bad trailer. The same decoder runs while waiting for
UART commands, without sending unsolicited records. The edge-only probe
temporarily takes the capture timers and does not decode frames. Use
`ram_trial.py --panel-after 30` to check LED activity before opening any
stream. A lit LED is not proof of delivery.
The DIP state is diagnostic, not permission to drive a bus. Test at
power-on retains the hardware ROM entry function.

### Hardware verification

| Interface | Repeatable check | What still requires a person or peer |
| --- | --- | --- |
| UART0 TX/RX and software ISP entry | An SRAM trial verifies the upload byte-for-byte, executes a command, returns through ROM and identifies the installed dev image. | Observe Power and UART TX/RX lamps during the transfer to check the lamps themselves. |
| Five tN4 lines | Use `--echo-test` and `--frames 35 --diagnostics` in separate trials. Inspect per-port results, input levels and edge counts. | End-to-end verification needs an active peer on each physical port. A permanently dominant input is never driven. |
| Five bus LEDs and Test LED | `--panel-after 25 --panel-test` reads idle activity, then follows the six individual output pin levels. | Watch Bus 0 through 4, then Test. Pin readback alone cannot detect a failed lamp or a swapped label. |
| Test button and Reporting DIP | `board.py panel PORT` reports raw-at-boot and debounced states plus change counters. The ARM tests simulate switch bounce and verify a single press/change. | To verify the physical controls, observe the changed counter after a press or switch change with the panel build running. |

The [482 installation manual, pp. 8-10](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/42923-source/638820695670000000/482-d-09.pdf)
identifies DTR as programming-only, and the DIP as the stock reporting
power-on default. Neither is an application flow-control signal.
