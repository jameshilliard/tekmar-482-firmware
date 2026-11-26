//! Passive tN4 input capture. No bus output or VIC source is enabled.
use super::{PCONP, PINSEL0, read, send, write};

const TIMERS: [usize; 2] = [0xe000_4000, 0xe000_8000];
const IR: usize = 0;
const TCR: usize = 0x04;
const TC: usize = 0x08;
const PR: usize = 0x0c;
const MCR: usize = 0x14;
const CCR: usize = 0x28;
const CR0: usize = 0x2c;
const CTCR: usize = 0x70;
const TIMER_POWER: u32 = (1 << 1) | (1 << 2);
const FIO0MASK: usize = 0x3fff_c010;
const FIO0PIN: usize = 0x3fff_c014;

// Stock logical port order: (P0 pin, timer index, capture channel).
const INPUTS: [(u32, usize, usize); 5] = [(4, 0, 1), (11, 1, 1), (6, 0, 2), (10, 1, 0), (2, 0, 0)];

pub struct Capture {
    mux: u32,
    power: u32,
    mask: u32,
    initial: u32,
}

impl Capture {
    pub fn start() -> Self {
        Self::arm(false)
    }

    fn arm(alternate: bool) -> Self {
        let mut saved = Self {
            mux: read(PINSEL0),
            power: read(PCONP),
            mask: read(FIO0MASK),
            initial: 0,
        };
        write(PCONP, saved.power | TIMER_POWER);
        for t in TIMERS {
            write(t + TCR, 2); // stop and reset counter and prescaler
            write(t + MCR, 0);
            write(t + CCR, 0);
            write(t + CTCR, 0); // timer mode, not external event counter
            write(t + PR, 47); // 48 MHz peripheral clock -> microseconds
        }
        let mut mux = saved.mux;
        let mut mask = saved.mask;
        for (pin, _, _) in INPUTS {
            mux = (mux & !(3 << (2 * pin))) | (2 << (2 * pin));
            mask &= !(1 << pin);
        }
        write(PINSEL0, mux);
        write(FIO0MASK, mask);
        saved.initial = read(FIO0PIN);
        let mut captures = [0; 2];
        for (pin, timer, channel) in INPUTS {
            let flags = match (alternate, saved.initial & (1 << pin) != 0) {
                (false, _) => 7,
                (true, false) => 5, // currently low: capture the next rising edge
                (true, true) => 6,
            };
            captures[timer] |= flags << (3 * channel);
        }
        for (t, ccr) in TIMERS.into_iter().zip(captures) {
            write(t + IR, 0xff);
            write(t + CCR, ccr);
            write(t + TCR, 1);
        }
        saved
    }

    pub fn report(self) {
        for t in TIMERS {
            write(t + CCR, 0);
            write(t + TCR, 0);
        }
        let elapsed = TIMERS.map(|t| read(t + TC));
        let flags = TIMERS.map(|t| read(t + IR));
        let mut seen = 0;
        let mut stamps = [0; INPUTS.len()];
        for (port, (_, timer, channel)) in INPUTS.into_iter().enumerate() {
            if flags[timer] & (1 << (4 + channel)) != 0 {
                seen |= 1 << port;
                stamps[port] = read(TIMERS[timer] + CR0 + 4 * channel);
            }
        }
        drop(self); // release the peripherals before the potentially slow serial reply
        send(b"RX");
        for word in elapsed.into_iter().chain([seen]).chain(stamps) {
            super::send_word(word);
        }
        send(b"\r\n");
    }
}

mod stream;
pub use stream::{Receivers, stream};

impl Drop for Capture {
    fn drop(&mut self) {
        for t in TIMERS {
            write(t + CCR, 0);
            write(t + TCR, 0);
            write(t + IR, 0xff);
        }
        write(PINSEL0, self.mux);
        write(FIO0MASK, self.mask);
        write(
            PCONP,
            (read(PCONP) & !TIMER_POWER) | (self.power & TIMER_POWER),
        );
    }
}
