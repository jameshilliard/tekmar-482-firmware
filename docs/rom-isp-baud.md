# LPC2146 ROM ISP baud rates on the tekmar 482

**Recommended: 187500 baud, 8N1, from the first `?` onward. Do not send
`B 187500 1`.** The ROM can synchronize directly at this speed, but rejects
it as an argument to the Set Baud Rate command. For a standard host rate,
38400 is the fastest of the six documented rates that worked without changing
speed on this board.

Bench: 12 MHz tekmar 482, LPC2146 part ID `0x0402ff23`, ROM 2.13, Linux,
Prolific USB-RS232 adapter, serialx 1.8.1. ISP was entered from the running
application at 9600 baud; the host then selected the
test baud **before** sending the ROM's first `?`. These are host settings,
not oscilloscope measurements or specifications for other boards.

## All documented ROM settings

The first column lists every argument in the [NXP LPC214x manual, Table 295](https://www.nxp.com/docs/en/user-guide/UM10139.pdf#page=306).
The ROM acknowledged all six `B <rate> 1` commands at the previous baud.

| Nominal baud | Direct handshake and transfer at nominal host baud | Same-speed direct alternative tested | Host baud(s) verified after `B` |
| ---: | :--- | ---: | :--- |
| 9600 | Pass | 9375 | 9600, 9375 |
| 19200 | Pass | 18750 | 19200, 18750 |
| 38400 | Pass | 37500 | 38400, 37500 |
| 57600 | No valid handshake | 62500 | 62500; no part ID at 57600 |
| 115200 | No valid handshake | 93750 | 93750; no part ID at 115200 |
| 230400 | No valid handshake | 187500 | 187500; no part ID at 230400 |

For each of the last three rows, the ROM did detect the initial `?` at the
nominal speed: the host could complete that *same pending handshake* by
answering `Synchronized` at the alternative speed. Separately, starting at
the alternative speed worked from handshake through data transfer with no
speed change.

The ROM returned status 17 (invalid baud rate) for `B` with 4800, 9375,
9601, 18750, 31250, 37500, 62500, 93750 and 187500. The six-command table
and the set of working direct autobaud rates are not interchangeable.

## Additional direct autobaud rates

All 20 candidates of the form `round(187500 / n)` for `n = 1..20` passed
at one fixed host rate. This clock-divider model describes the observed
rates; it does not mean the ROM rejects nearby host rates.

| Divisors (in order) | Verified host baud (in order) |
| --- | --- |
| 1-5 | 187500, 93750, 62500, 46875, 37500 |
| 6-10 | 31250, 26786, 23438, 20833, 18750 |
| 11-15 | 17045, 15625, 14423, 13393, 12500 |
| 16-20 | 11719, 11029, 10417, 9868, 9375 |

Additional fixed host rates **300, 600, 1200, 2400, 4800, 7200 and 14400**
also passed the direct handshake and 96-byte transfer tests. This is a
complete table of the documented `B` settings, not an exhaustive sweep of
every baud value a host driver can request or the tolerance around each rate.

## Verification

- Direct pass: fresh `?`, synchronization and crystal exchange, part ID,
  random data written to scratch SRAM at `0x40001000` and read back
  byte-for-byte, and flash read at `0x1000` compared with the saved backup.
  These transfers used 1024 bytes each for the six main working host rates
  (9600, 19200, 38400, 62500, 93750, 187500) and 96 bytes each for the
  additional probe rates. No flash erase or program command was used.
- Direct uploads used no flow control and waited one encoded line's wire time
  plus 2 ms between UU-encoded lines. The earlier 187500 bulk test also
  verified three 4 KiB SRAM uploads/readbacks and 48 KiB of flash reads.
- After `B`, short commands working did not guarantee a successful upload.
  With only line pacing, uploads at higher settings timed out or received
  `RESEND`. With an additional 1.5 ms delay between encoded bytes, the
  post-`B` rates listed above for settings 19200 and higher all passed the
  1 KiB SRAM write/readback and flash comparison. This was a diagnostic
  throttle, not a measured minimum delay or a reason to use `B`.
