# tekmar 482 development firmware

A standalone Rust/Embassy image for the LPC2146-based 482 with the 12 MHz
crystal. Installing it replaces the factory firmware. UART0 carries [binary TPCK packets](docs/tpck-bridge.md) for tRPC management,
passive tN4 capture, single-attempt sends and entry into ROM ISP.

## Build and test on Linux

Install rustup, GNU Make, Python 3, uv and GNU Arm binutils (Debian package
`binutils-arm-none-eabi`). From this directory:

```sh
make check
```

Rustup selects stable Rust 1.100.0 and supplies matching rust-src. The
[ARMv4T bare-metal target](https://doc.rust-lang.org/rustc/platform-support/armv4t-none-eabi.html)
needs Cargo to build core from source. This still uses the unstable
`-Zbuild-std=core` Cargo option: the Makefile sets `RUSTC_BOOTSTRAP=1` only
on cross-build and cross-Clippy commands, not globally. The production
runtime uses the Embassy revision pinned in Cargo.lock. The Rust application
uses Thumb-1 for compact code in flash. Reset
vectors, interrupt masking and the ROM handoff use ARM instructions.
The release profile uses `opt-level = "s"`, full LTO, a single codegen
unit and abort-on-panic. Compare **revision-stamped binaries** when tuning:
`"s"` currently produces a smaller image than `"z"` on the pinned toolchain.
Debug information stays in the ELF, not in the binary loaded on the device.
`make image` alone builds the ELF, binary and Intel HEX under
`target/thumbv4t-none-eabi/release/tekmar-482-firmware{,.bin,.hex}`. The linker
limits flash to the first 64 KiB. The build checks the LPC vector checksum,
ARM entry addresses, code read protection word, size and HEX round trip.
`make check` exercises the parser and runs that flash image at both 9600
and 187500 baud in the emulator. It tests all five capture/TX paths, UART
backpressure, panel deadlines, stack headroom and the handoff to ROM.
Neither compilation nor emulation writes a physical device.

Use flash for routine hardware tests as well. Build the Python client's
sdist with this firmware revision pinned, then use its
[`tekmar-482 update` procedure](https://github.com/jameshilliard/tekmar-482#installation-and-updates).
This skips an already-current build and verifies the revision after an update.

### Execution model

The ARM7TDMI port uses Embassy's cooperative thread executor, ARM7 IRQ/FIQ
critical sections and portable atomics. The bridge task handles UART
packet framing, mode changes, streaming and ROM handoff. It captures idle bus traffic
while waiting on commands and replies. A separate timed task debounces the
panel inputs and updates the indicators even while the bridge awaits UART
space.

Embassy time uses the free-running PWM counter at 1 MHz and extends its
32-bit ticks across wraps. Timer0 and Timer1 remain dedicated to tN4 edge
capture and may restart on mode changes without moving Embassy time.
Peripheral readiness is polled, not VIC interrupt driven: the executor
spins and streaming yields after each bounded service pass. Link ACK
slots and transmit edges are driven from the capture timers, not from
Embassy task delays.

`make check-ram` optionally builds and emulates an SRAM-linked image at
`0x40001000` while the firmware fits its 24 KiB region. This is not a CI or
release requirement. `make ram-trial` only builds it. Its binary is under
`target/ram-trial/thumbv4t-none-eabi/release/`.
This is **not a flash image**. On an unprotected part, after verifying a backup,
ISP can write the binary to RAM, read it back for comparison, and use Go in ARM
mode at `0x40001000`. This exercises the protocol on real hardware without any
flash erase/program command. Power cycling normally returns to the application
installed in flash, not the RAM trial.

The Python host tools use asyncio and serialx. In a fresh ISP session,
`uv run --script tools/backup.py PORT STOCK.bin NEW_DIRECTORY` reads user
flash twice at 9600 baud, verifies the data checksums and checks the chip ID.
Use a flat binary converted from the correct vendor HEX. A separate restore
image uses its first 64 bytes only when the rest of the vendor application
matches both reads; both raw reads are retained. This helper never unlocks,
erases or programs the device.

After the backup finishes successfully, the ROM remains synchronized at 9600
with echo off. In that same session (no power cycle), run
`uv run --script tools/ram_trial.py PORT target/ram-trial/thumbv4t-none-eabi/release/tekmar-482-firmware.bin`.
The helper rejects flash-linked images, checks the part ID, uploads to SRAM
only, reads it back, and runs the verified bytes. It checks the application
identity, requests ISP and checks the part ID again in the new ROM session.
Power-cycle normally after the trial. No erase, flash copy or persistent
installation is part of this test.

## Bench use

For the `board.py frames`, `probe`, and `send` commands,
see the [interface map and capture procedure](docs/interfaces.md).
For frame headers, checksum, payloads and the 483 message-ID catalog, see
the [native tN4 wire reference](docs/tn4-frames.md).

Use a **spare gateway disconnected from the tN4 buses**. Before the first
write, retain a verified backup and the stock HEX for that unit's lot.
Close any process using the serial adapter. Initial installation from
stock still requires [tekmar's Test-button ISP procedure](https://www.watts.com/our-story/brands/tekmar/482firmware).
Use the built `.hex`, erase only needed sectors, verify the write, then
power-cycle without Test. Neither the build nor the helper flashes a device.

```sh
PORT='/dev/serial/by-id/<your-adapter-id>'
uv run --script tools/board.py identify "$PORT"
uv run --script tools/board.py enter-isp "$PORT"
```

The application starts in Normal mode at 9600 baud, 8N1, without flow
control. A host at that rate can send its first TPCK request immediately. At
other fixed rates the helper uses [TPCK baud acquisition](docs/tpck-bridge.md#baud-acquisition).
The helper
uses BridgeCapabilities to identify the application. FirmwareBuild reports
the full Git commit ID when built with `TEKMAR_BUILD_REVISION`, otherwise
`unversioned`. See the [wire reference](docs/tpck-bridge.md) for the
encoded requests and the limits of Normal-mode support.

The firmware update request drains its binary reply, returns the peripheral
clock to the 12 MHz crystal and invokes ROM ISP. The ROM has its own
handshake: select the programming baud before sending its initial `?`.
The helper never erases or programs flash. To verify ROM entry without a
write, use `lpc21isp -detectonly "$PORT" 9600 12000` and check for LPC2146
part ID `0x0402ff23`. Power cycling without Test restarts the application in
flash. Use physical ISP entry when the installed application does not
support the handoff. Only trusted clients should have access to this link.

Register and boot contracts: [NXP UM10139](https://www.nxp.com/docs/en/user-guide/UM10139.pdf),
chapters 4 and 9, and sections 21.4.2, 21.4.11-12 and 21.9.8.

### Fixed baud for ROM ISP

On the 12 MHz gateway with ROM 2.13 and the Prolific USB-RS232 adapter,
use **187500 baud, 8N1** from the initial `?` through the last ISP command.
The ROM accepts this rate by autobaud; do not send a `B` (Set Baud Rate)
command. This is a bench-tested rate for this combination, not a guarantee
for other clocks or adapters.

See the [ROM ISP baud table](docs/rom-isp-baud.md) for all six documented settings,
direct autobaud tests, and the host rates that worked after a baud change.

At that fixed rate, three 4096-byte SRAM uploads read back byte-for-byte and
48 KiB of flash matched the backup. Back-to-back uploads elicited `RESEND`
even with XON/XOFF enabled. The passing upload used no flow control, waited
one line's wire time (10 bits per byte, including CRLF) plus 2 ms after each
UU-encoded line, and checked each block's checksum reply.
