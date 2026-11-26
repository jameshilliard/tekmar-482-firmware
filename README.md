# tekmar 482 development firmware

A minimal standalone Rust image for the LPC2146-based 482 with the 12 MHz
crystal. Installing it replaces the factory firmware. The development image
uses UART0 for identification, streaming passively decoded tN4 frames, and entry
into ROM ISP. It does not transmit on tN4 or implement tRPC.

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
on cross-build and cross-Clippy commands, not globally. There are no
third-party firmware crates.
`make image` alone builds the ELF, binary and Intel HEX under
`target/armv4t-none-eabi/release/tekmar-482-firmware{,.bin,.hex}`. The linker
limits flash to the first 64 KiB. The build checks the LPC vector checksum,
ARM entry addresses, code read protection word, size and HEX round trip.
`make check` also exercises the parser and runs the built image against a
simulated UART up to the ROM entry point, not through the NXP ROM itself.

`make ram-trial` builds the same prototype linked to run at SRAM address
`0x40001000`. Its binary is under `target/ram-trial/armv4t-none-eabi/release/`.
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
`uv run --script tools/ram_trial.py PORT target/ram-trial/armv4t-none-eabi/release/tekmar-482-firmware.bin`.
The helper rejects flash-linked images, checks the part ID, uploads to SRAM
only, reads it back, and runs the verified bytes. It checks the application
identity, requests ISP and checks the part ID again in the new ROM session.
Power-cycle normally after the trial. No erase, flash copy or persistent
installation is part of this test.

## Bench use

For the receive-only bridge and the `board.py frames` and `probe` commands,
see the [interface map and capture procedure](docs/interfaces.md).

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

The autobaud firmware (`TEKMAR-482-DEV`) uses 8N1, no flow control.
Prefix **each** application command with ASCII `A\n`: send `A\nID?\n` to
read the identity, or `A\nENTER-ISP\n` to receive `OK ISP\r\n` and enter
ROM ISP. The prefix is exactly A then LF; the command accepts LF or CRLF.
Wait for the reply before sending another command. The host helper sends
the prefix automatically.

UART0 measures the first two falling edges of the A using hardware autobaud
mode 0 ([UM10139 section 10.3.13](https://www.nxp.com/docs/en/user-guide/UM10139.pdf#page=158)).
PLL0 supplies a 48 MHz peripheral clock during application operation. The
firmware drains the final ACK, restores the 12 MHz crystal clock, then calls
IAP command 57 with interrupts masked. **The ROM has its own handshake**:
send a bare `?` at the selected speed, not the application's `A\n` prefix.

Use `--baud 187500` on `board.py` to use one baud from identification through
the ROM session.
The ISP command is unauthenticated: anyone with access to that serial link
can reach the ROM programmer. Do not expose a serial bridge to untrusted users.

The helper verifies the firmware identity before sending the command and
does not send any programming traffic. After it exits, start an LPC2146 ISP
tool on the same serial port, with its oscillator parameter set to 12000 kHz.
For a non-writing bench test, use `lpc21isp -detectonly "$PORT" 9600 12000`
and expect LPC2146 (part ID `0x0402FF23`). That test, unlike the ASCII ACK,
confirms that the ROM actually answered. Power-cycle without Test to leave
ISP and restart the unchanged application. Physical Test-button recovery
remains available if the custom application cannot boot.

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
