//! Gateway wire timestamps from the free-running 1 MHz panel timer. The
//! gateway samples it every pass, well inside its 71-minute wrap period.
const MICROS_PER_SECOND: u32 = 1_000_000;
const MILLIS_PER_SECOND: u32 = 1000;
const MICROS_PER_MILLI: u32 = MICROS_PER_SECOND / MILLIS_PER_SECOND;
#[derive(Default)]
pub struct Uptime {
    epoch_us: u32,
    pub seconds: u32,
}

impl Uptime {
    pub fn sample(&mut self, us: u32) {
        let elapsed = us.wrapping_sub(self.epoch_us);
        if elapsed >= MICROS_PER_SECOND {
            let seconds = elapsed / MICROS_PER_SECOND;
            self.epoch_us = self.epoch_us.wrapping_add(seconds * MICROS_PER_SECOND);
            self.seconds = self.seconds.wrapping_add(seconds);
        }
    }

    pub fn millis(&self, us: u32) -> u32 {
        self.seconds
            .wrapping_mul(MILLIS_PER_SECOND)
            .wrapping_add(us.wrapping_sub(self.epoch_us) / MICROS_PER_MILLI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_and_wire_milliseconds_survive_both_timer_wraps() {
        let mut uptime = Uptime::default();
        for i in 0..4000u64 {
            let us = i * 1_234_567_891;
            uptime.sample(us as u32);
            assert_eq!(uptime.seconds, (us / 1_000_000) as u32);
            assert_eq!(uptime.millis(us as u32), (us / 1000) as u32);
        }
    }
}
