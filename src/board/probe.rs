//! Passive tN4 input capture. No bus output or VIC source is enabled.
use super::{
    PCONP, PINSEL0,
    lpc2146::{PRESCALE_MICROSECONDS, gpio, timer},
    read, write,
};
use tekmar_482_firmware::tn4::PORT_COUNT;

const TIMERS: [usize; 2] = [0xe000_4000, 0xe000_8000];
const IR: usize = 0;
use timer::{CONTROL as TCR, COUNTER as TC, MATCH_CONTROL as MCR, PRESCALE as PR};
const CAPTURE_PIN_FUNCTION: u32 = 2;
const CCR: usize = 0x28;
const CR0: usize = 0x2c;
const CTCR: usize = 0x70;
const TIMER_POWER: u32 = (1 << 1) | (1 << 2);
const FIO0MASK: usize = gpio::FIO0 + gpio::MASK;
const FIO0PIN: usize = gpio::FIO0 + gpio::PIN;

#[derive(Clone, Copy)]
struct CaptureInput {
    pin: u32,
    timer: usize,
    channel: usize,
}
impl CaptureInput {
    const fn new(pin: u32, timer: usize, channel: usize) -> Self {
        Self {
            pin,
            timer,
            channel,
        }
    }
    fn timestamp(self) -> u32 {
        read(TIMERS[self.timer] + CR0 + size_of::<u32>() * self.channel)
    }
}
// Physical P0 pin, peripheral timer and CAP channel in gateway port order.
const INPUTS: [CaptureInput; PORT_COUNT] = [
    CaptureInput::new(4, 0, 1),
    CaptureInput::new(11, 1, 1),
    CaptureInput::new(6, 0, 2),
    CaptureInput::new(10, 1, 0),
    CaptureInput::new(2, 0, 0),
];

fn next_edge_flags(high: bool) -> u32 {
    timer::CAPTURE_IRQ
        | if high {
            timer::CAPTURE_FALLING
        } else {
            timer::CAPTURE_RISING
        }
}

pub struct Capture {
    mux: u32,
    power: u32,
    mask: u32,
    initial: u32,
}

impl Capture {
    fn arm() -> Self {
        let mut saved = Self {
            mux: read(PINSEL0),
            power: read(PCONP),
            mask: read(FIO0MASK),
            initial: 0,
        };
        write(PCONP, saved.power | TIMER_POWER);
        for t in TIMERS {
            write(t + TCR, timer::RESET); // stop and reset counter and prescaler
            write(t + MCR, 0);
            write(t + CCR, 0);
            write(t + CTCR, 0); // timer mode, not external event counter
            write(t + PR, PRESCALE_MICROSECONDS); // 48 MHz peripheral clock -> microseconds
        }
        let mut mux = saved.mux;
        let mut mask = saved.mask;
        for CaptureInput { pin, .. } in INPUTS {
            mux = (mux & !(gpio::MUX_MASK << (gpio::MUX_BITS as u32 * pin)))
                | (CAPTURE_PIN_FUNCTION << (gpio::MUX_BITS as u32 * pin));
            mask &= !(1 << pin);
        }
        write(PINSEL0, mux);
        write(FIO0MASK, mask);
        saved.initial = read(FIO0PIN);
        let mut captures = [0; TIMERS.len()];
        for input in INPUTS {
            let flags = next_edge_flags(saved.initial & (1 << input.pin) != 0);
            captures[input.timer] |= flags << (timer::CAPTURE_FIELD_BITS * input.channel);
        }
        for (t, ccr) in TIMERS.into_iter().zip(captures) {
            write(t + IR, timer::CLEAR_ALL_IRQS);
            write(t + CCR, ccr);
            write(t + TCR, timer::ENABLE);
        }
        saved
    }
}

mod stream;
pub use stream::run;

impl Drop for Capture {
    fn drop(&mut self) {
        for t in TIMERS {
            write(t + CCR, 0);
            write(t + TCR, 0);
            write(t + IR, timer::CLEAR_ALL_IRQS);
        }
        write(PINSEL0, self.mux);
        write(FIO0MASK, self.mask);
        write(
            PCONP,
            (read(PCONP) & !TIMER_POWER) | (self.power & TIMER_POWER),
        );
    }
}
