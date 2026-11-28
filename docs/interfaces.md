# Bridge interface bring-up

The LPC2146 has a UART0 host link and five tN4 receive paths. The table
uses the logical port numbers in the stock J1196J sampling routines, not
thermostat addresses. In Raw mode only explicit TX and receive-ACK commands
can drive a bus; Normal mode also issues its own native requests.

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

Use `uv run --script tools/board.py probe "$PORT"` at 9600 baud. The
helper reads the capture counters twice, one second apart, and displays
edge counts and the last latched timer value for each port. The 1 MHz
capture timers wrap after about 71 minutes. A quiet input is not a failed
input. A hardware capture latch is not an edge FIFO.

## Stream decoded frames

```sh
uv run --script tools/board.py frames "$PORT" --seconds 60
uv run --script tools/board.py diagnostics "$PORT" --seconds 60 --save-capture capture.tpck
python3 tools/frames.py capture.tpck
```

Run these checks directly on the installed flash image. For an optional
SRAM trial of a build that still fits the 24 KiB window:

```sh
make ram-trial
uv run --script tools/ram_trial.py "$PORT" \
  target/ram-trial/thumbv4t-none-eabi/release/tekmar-482-firmware.bin \
  --from-app --frames 60
```

The helper negotiates Raw mode, listens on all five lines and requests
Normal mode when finished. By default it neither sends a frame nor
acknowledges incoming traffic. The archive contains binary TPCK Raw
records: the same decoder handles live and replayed traffic. Invalid
captures are diagnostics, not device updates. See the [packet layouts and
ordering](tpck-bridge.md) and the [native tN4 payload catalog](tn4-frames.md).

To hand the UART to another host using the 485 native format without the
482 result and diagnostic records, run `board.py raw-base PORT`. Run
`board.py normal PORT` to restore Normal mode after that host disconnects.
Use the same fixed UART rate for both hosts.

### Receiver diagnostics

The gateway counts valid and invalid records dropped under UART
backpressure separately per physical port. The counters persist across
capture windows; compare snapshots to measure drops during a window.
They do not count missed electrical edges. Invalid captures include a
reason, bit count and the captured bits, so a host can inspect a bad
frame without interpreting it as an application message.

The optional `diagnostics --ack-port N` enables link ACK on just that
port in Raw mode. Normal mode already acknowledges peers on each line.
The gateway asserts the ACK slot only after a matching CRC and a
valid CRC delimiter. This is useful on a two-device line. A link ACK does
not confirm that a device applied a setting.

### One-shot transmit

`board.py send PORT --tx BUS HEX` takes a complete native header and
payload, including the source chosen by the host. The firmware waits for
an idle line, adds stuffing, CRC and trailer, and makes one attempt.
Check the terminal result and use an addressed readback to verify a
device setting. A host deadline is not a cancellation. A line that
never goes idle can retain the pending send.

`board.py echo-test PORT --bus N` sends a block read with an empty target
bitmap. The helper checks for a matching echo on the selected port. This
checks the local driver and capture input, not field wiring. A peer ACK
is not required for this test.

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
are active low. A button press starts the [onboard Test sequence](normal-mode.md#gateway-diagnostics).
The separate `board.py led-test PORT` walks bus 0..4 then Test, 500 ms
each, without simulating a press or sending version reports. The host
checks all six pin readbacks and that the walk ends. The RAM trial
accepts `--panel-test`. Outside the walk, a bus
LED denotes a CRC-matching frame seen in the last minute, including an
intact body with a bad trailer. The same decoder runs while waiting for
UART commands. Use
`ram_trial.py --panel-after 30` to check LED activity before opening any
stream. A lit LED is not proof of delivery.
The DIP state is diagnostic, not permission to drive a bus. Test at
power-on retains the hardware ROM entry function.

For a witnessed test on the installed flash firmware:

```sh
uv run --script tools/board.py panel "$PORT" --check-input test --seconds 90
```

Wait for the prompt before touching the control. This test reads panel state without starting a software lamp test. It requires the debounced button
to be observed pressed and released, exactly one new press, and the
Test LED pin to be active. The person at the panel checks the actual light.
The corresponding `--check-input dip` test needs both switch positions
to be observed and exactly two transitions back to the original setting.
Use only controls accessible with the enclosure closed; skip a physical
test if it requires opening an energized enclosure. A timeout is not a
pass. This check leaves the installed application running.

### Hardware verification

| Interface | Repeatable check | What still requires a person or peer |
| --- | --- | --- |
| UART0 TX/RX and software ISP entry | An SRAM trial verifies the upload byte-for-byte, executes a command, returns through ROM and identifies the installed dev image. | Observe Power and UART TX/RX lamps during the transfer to check the lamps themselves. |
| Five tN4 lines | Use `--echo-test` and `--frames 35 --diagnostics` in separate trials. Inspect per-port results and drop counters; use the probe to check edge counts. | End-to-end verification needs an active peer on each physical port. A permanently dominant input is never driven. |
| Five bus LEDs and Test LED | `--panel-after 25 --panel-test` reads idle activity, then follows the six individual output pin levels. | Watch Bus 0 through 4, then Test. Pin readback alone cannot detect a failed lamp or a swapped label. |
| Test button and Reporting DIP | On the running panel build, use `board.py panel PORT --check-input test` or `--check-input dip`. | An operator must act on the physical control. Counters alone do not pass either test. |

An earlier bring-up image used a button press to start the lamp sweep.
That trial verified the six visible lights and their pin levels, not the
Normal-mode Test sequence in the current image.

The [482 installation manual, pp. 8-10](https://www.watts.com/dfsmedia/0533dbba17714b1ab581ab07a4cbb521/42923-source/638820695670000000/482-d-09.pdf)
identifies DTR as programming-only, and the DIP as the stock reporting
power-on default. Neither is an application flow-control signal.
