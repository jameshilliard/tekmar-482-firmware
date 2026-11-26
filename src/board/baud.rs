//! Optional cold acquisition. At the vendor rate the first packet passes unchanged.
use super::{ACR, UART0, read, write};
use tekmar_482_firmware::tpck::Decoder;

pub(crate) const DEFAULT_DIVISOR: u32 = 313;

pub(crate) fn set_divisor(divisor: u32) {
    write(ACR, 0x300);
    write(UART0 + 0x0c, 0x83);
    write(UART0, divisor & 255);
    write(UART0 + 4, divisor >> 8);
    write(UART0 + 0x0c, 3);
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
            if gap >= 15_000 {
                self.pair = (40_000..=160_000).contains(&gap);
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
        if self.measuring.is_none() && read(0x3fff_c014) & (1 << 1) == 0 {
            self.activity(now);
        }
        if let Some(start) = self.measuring {
            if read(ACR) & 1 == 0 {
                write(UART0 + 0x0c, 0x83);
                let measured = (read(UART0 + 4) << 8) | read(UART0);
                // Mode 1 measures start to first rising edge. For TPCK CA
                // (LSB first), start and bit zero are both low: two bit times.
                // The measured divider can include one extra count (33 at
                // 187500 baud). Rounding it up again selects 17 instead of 16.
                set_divisor((measured / 2).clamp(1, 65535));
                self.tentative = Some(now);
            } else if now.wrapping_sub(start) > 300_000 {
                set_divisor(DEFAULT_DIVISOR);
            } else {
                return;
            }
            self.measuring = None;
            self.last_byte = None;
            parser.reset();
        } else if self
            .tentative
            .is_some_and(|t| now.wrapping_sub(t) > 600_000)
        {
            set_divisor(DEFAULT_DIVISOR);
            self.tentative = None;
            self.last_byte = None;
            parser.reset();
        } else if self.pair && self.last_byte.is_some_and(|t| now.wrapping_sub(t) > 16_000) {
            write(ACR, 0x303); // start mode 1, clear previous completion flags
            self.measuring = Some(now);
            self.pair = false;
            parser.reset();
        }
    }
}
