//! Gateway wire timestamps from the free-running 1 MHz panel timer. The
//! gateway samples it every pass, well inside its 71-minute wrap period.
#[derive(Default)]
pub struct Uptime {
    epoch_us: u32,
    pub seconds: u32,
}

impl Uptime {
    pub fn sample(&mut self, us: u32) {
        let elapsed = us.wrapping_sub(self.epoch_us);
        if elapsed >= 1_000_000 {
            let seconds = elapsed / 1_000_000;
            self.epoch_us = self.epoch_us.wrapping_add(seconds * 1_000_000);
            self.seconds = self.seconds.wrapping_add(seconds);
        }
    }

    pub fn millis(&self, us: u32) -> u32 {
        self.seconds
            .wrapping_mul(1000)
            .wrapping_add(us.wrapping_sub(self.epoch_us) / 1000)
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
