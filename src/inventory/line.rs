//! Per-line Normal-mode discovery, fault accounting and transmit backoff.
use crate::tn4::tx::{Outcome, Request};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineProbe {
    MasterQuery = 0x3f7,
    NetworkInit = 0x3df,
}

impl LineProbe {
    pub fn frame(self, port: u8) -> Option<Request> {
        let header = (1 << 30) | ((self as u32) << 20);
        Request::new(port, &header.to_be_bytes())
    }
}

#[derive(Clone, Copy, Default)]
enum Init {
    #[default]
    Cold,
    AfterLinkUp(u32),
    Pending,
    Done,
}

#[derive(Clone, Copy, Default)]
pub struct NormalLine {
    up: bool,
    sampled_up: bool,
    query: bool,
    errors: u8,
    init: Init,
    backoff_us: u32,
    idle_since: Option<u32>,
}

// Stock 0x57e4 decrements 0x643 times in idle sample phase only.
// The timer match is 0x27 ticks at 16 us each (0x7f24).
const BACKOFF_US: u32 = 0x643 * 0x27 * 16;

impl NormalLine {
    pub(super) const COLD: Self = Self {
        up: false,
        sampled_up: false,
        query: false,
        errors: 0,
        init: Init::Cold,
        backoff_us: 0,
        idle_since: None,
    };

    pub(super) fn peer(&mut self, message: u32) {
        self.up = true;
        self.errors = 0;
        if message == LineProbe::NetworkInit as u32 {
            self.init = Init::Done;
        }
    }

    pub(super) fn tick(&mut self, now: u32) {
        if self.up && !self.sampled_up {
            self.query = true;
            self.init = Init::AfterLinkUp(now);
        } else if matches!(self.init, Init::Cold)
            || matches!(self.init, Init::AfterLinkUp(since) if now.wrapping_sub(since) >= 30)
        {
            self.init = Init::Pending;
        }
        self.sampled_up = self.up;
    }

    pub fn pending(&self) -> Option<LineProbe> {
        if self.query {
            Some(LineProbe::MasterQuery)
        } else if matches!(self.init, Init::Pending) {
            Some(LineProbe::NetworkInit)
        } else {
            None
        }
    }

    /// Arbitration can return this probe to the queue; retire it only on a terminal result.
    pub fn finished(&mut self, probe: LineProbe) {
        if self.pending() != Some(probe) {
            return; // A peer or a newer link-up already changed the pending action.
        }
        match probe {
            LineProbe::MasterQuery => self.query = false,
            LineProbe::NetworkInit => self.init = Init::Done,
        }
    }

    pub fn down(&mut self) {
        self.up = false;
        self.errors = 0;
    }

    pub fn start_backoff(&mut self) {
        self.backoff_us = BACKOFF_US;
        self.idle_since = None;
    }

    pub fn may_start(&mut self, now: u32, idle: bool) -> bool {
        if self.backoff_us != 0 {
            if idle {
                if let Some(prev) = self.idle_since.replace(now) {
                    self.backoff_us = self.backoff_us.saturating_sub(now.wrapping_sub(prev));
                }
            } else {
                self.idle_since = None;
            }
        }
        self.backoff_us == 0
    }

    pub fn frame_error(&mut self) {
        // J1196J uses the high nibble of this same count for missed ACKs.
        self.errors += 1;
        if self.errors >= 10 {
            self.down();
            self.start_backoff();
        }
    }

    pub fn sent(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Ack => {
                self.up = true;
                self.errors = 0;
            }
            Outcome::NoAck => {
                self.errors += 0x10;
                if self.errors >= 0x20 {
                    self.down();
                }
            }
            Outcome::EchoMismatch => {
                self.down();
                self.errors = 1; // drive fault resets the count before recording error 6
            }
            Outcome::BadEnd => self.frame_error(),
            Outcome::ArbitrationLost | Outcome::Late => {}
        }
    }
}
