# Bridge interface bring-up

The LPC2146 has a UART0 host link and five tN4 receive paths. The table
uses the logical port numbers in the stock J1196J sampling routines, not
thermostat addresses. The TX pins are listed for reference only.

| Port | RX pin | Capture input | TX pin (untouched) | Stock sample routine |
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
The receiver never asserts an ACK, sends a probe, or drives any bus.

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
| 0x17e | Snowmelt source, ordered key/value pairs; slab temperature (0x22), slab setpoint (0x23), edge temperature (0x24), keep-alive (0x2f) |
| 0x1f2 | I/O report subnet, node, channel, type and 16-bit value |
| 0x22f | Setpoint enable, setback, operating/demand/actuator/relay states, priorities, three temperatures and status excuses |
| 0x23f | DHW enable, setback, priorities, operating/demand/actuator states, target and tank temperatures |
| 0x24f | Plant device ID/count/type; subtype 6 (modulating boiler) state, pump, modulation levels, inlet/outlet raw values, output and return protection |
| 0x32f | Outdoor temperature; the 482 sends a second copy of the word (J1196J 0x3dd8) and reads the first (0x3e38). Show both even if they differ. |
| 0x337, 0x33f | Maximum boiler percent; air group and requested cycle length |
| 0x367 | Water subnet, stage, override, priority, pump flags and delay; target temperature only when output is on |
| 0x36f | Air group, mode, demand, schedule, output states, local/remote temperatures and target |
| 0x3c7 | Relay heartbeat water subnet, split into bus and control |
| 0x3cf | Supply, return, target and optional DHW supply temperatures, heat/cool mode, loop/pump percentages, setback and limit flags |
| 0x3d7 | Network error code, instance and priority |

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
