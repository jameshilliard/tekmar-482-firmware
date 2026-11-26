//! Normal-mode diagnostics started by the physical Test button (J1196J 0x342c).
use crate::trpc::{GatewayVersion, Reply, method};

const STAGE_US: u32 = 10_000_000;
const HELD_TIMEOUT_US: u32 = 300_000_000;
const FINISH_LOCKOUT_US: u32 = 3_000_000;
const DIP_SAMPLE_US: u32 = 1_000_000;
const HELD_BLINK_US: u32 = 500_000;

#[derive(Clone, Copy, Default)]
enum Stage {
    #[default]
    Idle,
    Running(GatewayVersion),
    Held(GatewayVersion),
    Finishing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Version(GatewayVersion),
    ReportingDip(bool),
}

impl Notice {
    pub fn method(self) -> u32 {
        match self {
            Self::Version(v) => v as u32,
            Self::ReportingDip(_) => method::REPORTING_STATE,
        }
    }

    pub fn report(self) -> Reply {
        match self {
            Self::Version(v) => Reply::report(self.method(), &v.word()),
            Self::ReportingDip(on) => Reply::report(self.method(), &[u8::from(on)]),
        }
    }
}

#[derive(Default)]
pub struct OnboardTest {
    stage: Stage,
    since: u32,
    sampled: u32,
    last_dip: bool,
}

impl OnboardTest {
    pub fn poll(&mut self, us: u32, pressed: bool, dip: bool, mut notify: impl FnMut(Notice)) {
        let elapsed = us.wrapping_sub(self.since);
        let next = match self.stage {
            Stage::Idle if pressed => Some(Stage::Running(GatewayVersion::Firmware)),
            Stage::Running(v) if pressed => Some(Stage::Held(v)),
            Stage::Running(v) if elapsed >= STAGE_US => Some(Self::advance(v)),
            Stage::Held(v) if pressed => Some(Self::advance(v)),
            Stage::Held(_) if elapsed >= HELD_TIMEOUT_US => Some(Stage::Idle),
            Stage::Finishing if elapsed >= FINISH_LOCKOUT_US => Some(Stage::Idle),
            _ => None,
        };
        if let Some(stage) = next {
            self.stage = stage;
            self.since = us;
            if let Stage::Running(v) = stage {
                notify(Notice::Version(v));
            }
        }
        if us.wrapping_sub(self.sampled) >= DIP_SAMPLE_US {
            self.sampled = us;
            // The DIP is a Test diagnostic, not the mutable reporting-enable flag.
            if matches!(self.stage, Stage::Running(_)) && self.last_dip != dip {
                self.last_dip = dip;
                notify(Notice::ReportingDip(dip));
            }
        }
    }

    fn advance(version: GatewayVersion) -> Stage {
        match version {
            GatewayVersion::Firmware => Stage::Running(GatewayVersion::Protocol),
            GatewayVersion::Protocol => Stage::Finishing,
        }
    }

    pub fn indicator(&self, us: u32) -> bool {
        match self.stage {
            Stage::Running(_) => true,
            Stage::Held(_) => (us.wrapping_sub(self.since) / HELD_BLINK_US) & 1 == 0,
            Stage::Finishing | Stage::Idle => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use GatewayVersion::{Firmware, Protocol};
    use Notice::{ReportingDip, Version};

    fn step(test: &mut OnboardTest, us: u32, press: bool, dip: bool, expect: &[Notice]) {
        let mut next = expect.iter();
        test.poll(us, press, dip, |notice| {
            assert_eq!(Some(&notice), next.next())
        });
        assert_eq!(next.next(), None);
    }

    #[test]
    fn timed_stages_across_timer_wrap_and_terminal_lockout() {
        let mut test = OnboardTest::default();
        let start = u32::MAX - 100_000;
        step(&mut test, start, false, true, &[]);
        step(&mut test, start, true, true, &[Version(Firmware)]);
        assert!(test.indicator(start));
        for (offset, dip, expect) in [
            (1_000_000, true, Some(ReportingDip(true))),
            (1_050_000, false, None),
            (2_000_000, false, Some(ReportingDip(false))),
            (9_999_999, false, None),
            (10_000_000, false, Some(Version(Protocol))),
            (19_999_999, false, None),
            (20_000_000, true, None),
        ] {
            step(
                &mut test,
                start.wrapping_add(offset),
                false,
                dip,
                expect.as_slice(),
            );
        }
        assert!(!test.indicator(start.wrapping_add(20_000_000)));
        step(&mut test, start.wrapping_add(22_999_999), true, true, &[]);
        step(&mut test, start.wrapping_add(23_000_000), false, true, &[]);
        step(
            &mut test,
            start.wrapping_add(23_000_001),
            true,
            true,
            &[Version(Firmware)],
        );
    }

    #[test]
    fn press_holds_stage_blinks_and_suppresses_dip_until_advancing() {
        let mut test = OnboardTest::default();
        step(&mut test, 0, true, false, &[Version(Firmware)]);
        step(&mut test, 10_000_000, true, true, &[]); // press wins over timeout
        assert!(test.indicator(10_499_999));
        assert!(!test.indicator(10_500_000));
        assert!(test.indicator(11_000_000));
        step(&mut test, 20_000_000, false, false, &[]);
        step(
            &mut test,
            21_000_000,
            true,
            true,
            &[Version(Protocol), ReportingDip(true)],
        );
        step(&mut test, 22_000_000, true, false, &[]);
        step(&mut test, 22_100_000, true, false, &[]);
        assert!(!test.indicator(22_100_000));
        step(&mut test, 25_100_000, false, false, &[]);
        step(&mut test, 26_100_000, true, true, &[Version(Firmware)]);
        // The DIP already reported true in the previous test. Do not send it again.
        step(&mut test, 27_100_000, false, true, &[]);
        step(&mut test, 28_100_000, true, false, &[]);
        step(&mut test, 328_100_000, false, false, &[]);
        assert!(!test.indicator(328_100_000));
        step(&mut test, 328_100_001, true, false, &[Version(Firmware)]);
    }
}
