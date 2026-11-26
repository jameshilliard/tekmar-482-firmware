//! The six stock active-low indicators; never configure a tN4 driver pin here.
use super::{PCONP, PINSEL0, read, write};

const SCS: usize = 0xe01f_c1a0;
const PINSEL2: usize = 0xe002_c014;
const FIO0: usize = 0x3fff_c000;
const FIO1: usize = FIO0 + 0x20;
const MASK: usize = 0x10;
const PIN: usize = 0x14;
const SET: usize = 0x18;
const CLR: usize = 0x1c;
const BUTTON: u32 = 1 << 14;
const DIP: u32 = 1 << 7;
const PINS: [u32; 6] = [22, 21, 18, 23, 25, 16];
const PWM: usize = 0xe001_4000;
const PWM_POWER: u32 = 1 << 5;
const STEP_US: u32 = 500_000;

fn pins(logical: u8) -> u32 {
    let mut value = 0;
    for (i, pin) in PINS.iter().enumerate() {
        if logical & (1 << i) != 0 {
            value |= 1 << pin;
        }
    }
    value
}

fn inputs() -> u8 {
    let low = !read(FIO0 + PIN);
    u8::from(low & DIP != 0) | (u8::from(low & BUTTON != 0) << 1)
}

pub(crate) fn now() -> u32 {
    read(PWM + 8)
}

pub struct Panel {
    saved_scs: u32,
    saved_mux: u32,
    saved_mux2: u32,
    saved_dir0: u32,
    saved_dir1: u32,
    saved_mask0: u32,
    saved_mask1: u32,
    saved_power: u32,
    last_sample: u8,
    stable: u8,
    boot: u8,
    presses: u32,
    dip_changes: u32,
    test_start: Option<u32>,
    activity: [Option<u32>; 5],
    lit: u8,
}

impl Panel {
    pub fn init() -> Self {
        let saved_scs = read(SCS);
        let saved_mux = read(PINSEL0);
        let saved_mux2 = read(PINSEL2);
        let saved_power = read(PCONP);
        write(SCS, saved_scs | 3);
        let saved_dir0 = read(FIO0);
        let saved_dir1 = read(FIO1);
        let saved_mask0 = read(FIO0 + MASK);
        let saved_mask1 = read(FIO1 + MASK);
        write(FIO0, saved_dir0 & !(DIP | BUTTON));
        write(PINSEL0, read(PINSEL0) & !((3 << 14) | (3 << 28)));
        write(FIO0 + MASK, read(FIO0 + MASK) & !(DIP | BUTTON));
        let leds = pins(63);
        write(FIO1 + MASK, read(FIO1 + MASK) & !leds);
        write(FIO1 + SET, leds); // preload inactive before enabling GPIO output
        write(PINSEL2, read(PINSEL2) & !(1 << 3)); // leave JTAG alone
        write(FIO1, read(FIO1) | leds);

        // Free-running timebase independent of the two tN4 capture timers.
        write(PCONP, read(PCONP) | PWM_POWER);
        write(PWM + 4, 2);
        write(PWM + 0x14, 0);
        write(PWM + 0x4c, 0); // no PWM outputs
        write(PWM + 0x0c, 47);
        write(PWM + 4, 1);
        let boot = inputs();
        Self {
            saved_scs,
            saved_mux,
            saved_mux2,
            saved_dir0,
            saved_dir1,
            saved_mask0,
            saved_mask1,
            saved_power,
            last_sample: boot,
            stable: boot,
            boot,
            presses: 0,
            dip_changes: 0,
            test_start: None,
            activity: [None; 5],
            lit: 0,
        }
    }

    fn show(&mut self, new: u8) {
        if new != self.lit {
            write(FIO1 + SET, pins(self.lit & !new));
            write(FIO1 + CLR, pins(new & !self.lit));
            self.lit = new;
        }
    }

    pub fn start_test(&mut self) {
        self.test_start = Some(now());
        self.show(1);
    }

    pub fn observe(&mut self, port: usize) {
        self.activity[port] = Some(now());
    }

    pub fn poll(&mut self) {
        let time = now();
        let sample = inputs();
        if sample == self.last_sample && sample != self.stable {
            if (sample ^ self.stable) & 1 != 0 {
                self.dip_changes = self.dip_changes.saturating_add(1);
            }
            if sample & !self.stable & 2 != 0 {
                self.presses = self.presses.saturating_add(1);
                self.test_start = Some(time);
            }
            self.stable = sample;
        }
        self.last_sample = sample;
        let mut mask = 0;
        for (port, seen) in self.activity.iter_mut().enumerate() {
            if seen.is_some_and(|at| time.wrapping_sub(at) < 60_000_000) {
                mask |= 1 << port;
            } else {
                *seen = None;
            }
        }
        if let Some(at) = self.test_start {
            let step = time.wrapping_sub(at) / STEP_US;
            if step < 6 {
                mask = 1 << step;
            } else {
                self.test_start = None;
            }
        }
        self.show(mask);
    }

    pub fn report(&self) -> [u32; 7] {
        let pin_state = !read(FIO1 + PIN);
        let mut actual = 0;
        for (i, pin) in PINS.iter().enumerate() {
            if pin_state & (1 << pin) != 0 {
                actual |= 1 << i;
            }
        }
        [
            u32::from(self.boot),
            u32::from(self.stable),
            self.presses,
            self.dip_changes,
            u32::from(self.lit),
            actual,
            u32::from(self.test_start.is_some()),
        ]
    }

    pub fn stop(&mut self) {
        self.show(0);
        write(PWM + 4, 0);
        write(
            PCONP,
            (read(PCONP) & !PWM_POWER) | (self.saved_power & PWM_POWER),
        );
        write(
            FIO1,
            (read(FIO1) & !pins(63)) | (self.saved_dir1 & pins(63)),
        );
        write(FIO0 + MASK, self.saved_mask0);
        write(FIO1 + MASK, self.saved_mask1);
        write(PINSEL2, self.saved_mux2);
        write(PINSEL0, self.saved_mux);
        write(
            FIO0,
            (read(FIO0) & !(DIP | BUTTON)) | (self.saved_dir0 & (DIP | BUTTON)),
        );
        // The ROM autobaud routine must regain the GPIO interface it started with.
        write(SCS, self.saved_scs);
    }
}
