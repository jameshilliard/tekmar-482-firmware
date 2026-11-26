//! Optional cold acquisition. At the vendor rate the first packet passes unchanged.
use super::{
    lpc2146::{PCLK_HZ, gpio, uart0},
    read, write,
};
use tekmar_482_firmware::tpck::Decoder;

const VENDOR_BAUD: u32 = 9_600;
const DIVISOR_CLOCK: u32 = uart0::CLOCKS_PER_BIT * VENDOR_BAUD;
pub(crate) const DEFAULT_DIVISOR: u32 = (PCLK_HZ + DIVISOR_CLOCK / 2) / DIVISOR_CLOCK;
const BURST_BREAK_US: u32 = 15_000;
const WAKE_PAIR_US: core::ops::RangeInclusive<u32> = 40_000..=160_000;
const WAKE_QUIET_US: u32 = 16_000;
const MEASURE_TIMEOUT_US: u32 = 300_000;
const CONFIRM_TIMEOUT_US: u32 = 600_000;
const TPCK_SOF_LOW_BITS: u32 = 2;

pub(crate) fn set_divisor(divisor: u32) {
    write(uart0::ACR, uart0::CLEAR_AUTOBAUD_FLAGS);
    write(uart0::LCR, uart0::FORMAT_8N1 | uart0::DLAB);
    write(uart0::DATA, divisor & u32::from(u8::MAX));
    write(uart0::DLM, divisor >> u8::BITS);
    write(uart0::LCR, uart0::FORMAT_8N1);
}

#[derive(Default)]
pub(super) struct Acquisition {
    last_byte: Option<u32>,
    pair: bool,
    measuring: Option<u32>,
    tentative: Option<u32>,
    pub locked: bool,
}

impl Acquisition {
    pub fn activity(&mut self, now: u32) {
        if let Some(prev) = self.last_byte {
            let gap = now.wrapping_sub(prev);
            if gap >= BURST_BREAK_US {
                self.pair = WAKE_PAIR_US.contains(&gap);
            }
        }
        self.last_byte = Some(now);
    }

    pub fn poll(&mut self, now: u32, parser: &mut Decoder) {
        if self.locked {
            return;
        }
        // Short bits may never enter the FIFO at the startup baud. Sample
        // RXD0 during the wake bursts; UART hardware times the final pulse.
        if self.measuring.is_none() && read(gpio::FIO0 + gpio::PIN) & gpio::RXD0 == 0 {
            self.activity(now);
        }
        if let Some(start) = self.measuring {
            if read(uart0::ACR) & uart0::AUTOBAUD_START == 0 {
                write(uart0::LCR, uart0::FORMAT_8N1 | uart0::DLAB);
                let measured = (read(uart0::DLM) << u8::BITS) | read(uart0::DATA);
                // Mode 1 measures start to first rising edge. For TPCK CA
                // (LSB first), start and bit zero are both low: two bit times.
                // The measured divider can include one extra count (33 at
                // 187500 baud). Rounding it up again selects 17 instead of 16.
                set_divisor((measured / TPCK_SOF_LOW_BITS).clamp(1, u32::from(u16::MAX)));
                self.tentative = Some(now);
            } else if now.wrapping_sub(start) > MEASURE_TIMEOUT_US {
                set_divisor(DEFAULT_DIVISOR);
            } else {
                return;
            }
            self.measuring = None;
            self.last_byte = None;
            parser.reset();
        } else if self
            .tentative
            .is_some_and(|t| now.wrapping_sub(t) > CONFIRM_TIMEOUT_US)
        {
            set_divisor(DEFAULT_DIVISOR);
            self.tentative = None;
            self.last_byte = None;
            parser.reset();
        } else if self.pair
            && self
                .last_byte
                .is_some_and(|t| now.wrapping_sub(t) > WAKE_QUIET_US)
        {
            write(
                uart0::ACR,
                uart0::CLEAR_AUTOBAUD_FLAGS | uart0::AUTOBAUD_START | uart0::AUTOBAUD_MODE_1,
            ); // start mode 1, clear previous completion flags
            self.measuring = Some(now);
            self.pair = false;
            parser.reset();
        }
    }
}
