# Embassy on the LPC2146

This standalone SRAM trial runs an unmodified Embassy executor and two timed
tasks on the ARM7TDMI. It is not part of the bridge image or its dependency
graph. The AArch32 and Cortex-M executor platforms target different cores;
this trial uses a spin pender, single-threaded polling, ARM7 IRQ/FIQ critical
sections and the portable-atomic fallback.

Timer0 supplies a 1 kHz clock. One task writes `F` to the existing ISP UART
at 100, 200 and 300 ms; the other writes `S` at 225 ms. The executor services
the timer queue by polling. At 400 ms the trial drains the UART and reenters
ROM ISP. It never selects tN4 pins or programs flash.

From the firmware repository root:

```sh
make embassy-trial
```

This builds the SRAM image, checks its vectors and runs it in the ARM emulator.
The emulator checks each wake time, the UART byte order and the ROM handoff.
For an optional physical trial starting from the custom development firmware:

```sh
uv run --script experiments/embassy/trial.py /dev/ttyUSBX \
    target/embassy/armv4t-none-eabi/release/lpc2146-embassy-trial.bin
```

The helper identifies the installed firmware and ROM part, reads back the
uploaded SRAM bytes, checks the `FFSF` response and restarts and identifies
the installed firmware. A power cycle also discards the trial.

This demonstrates timed async task execution on a physical LPC2146. It does
not establish interrupt latency, low-power wakeup, or loss-free tN4 capture.
The bridge already uses Timer0 for capture; a production time driver must
share that timebase without resetting it. An interrupt-driven executor and
timer match wakeup are separate work.
