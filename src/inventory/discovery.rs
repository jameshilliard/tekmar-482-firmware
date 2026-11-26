//! Per-line Normal-mode bring-up (J1196J application service at 0x78dc).
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
pub struct LineDiscovery {
    up: bool,
    sampled_up: bool,
    query: bool,
    errors: u8,
    init: Init,
}

impl LineDiscovery {
    pub(super) const COLD: Self = Self {
        up: false,
        sampled_up: false,
        query: false,
        errors: 0,
        init: Init::Cold,
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

    /// A waiting driver can yield to a host operation; consume the probe only at SOF.
    pub fn started(&mut self, probe: LineProbe) {
        match probe {
            LineProbe::MasterQuery => self.query = false,
            LineProbe::NetworkInit => self.init = Init::Done,
        }
    }

    pub fn down(&mut self) {
        self.up = false;
        self.errors = 0;
    }

    pub fn frame_error(&mut self) {
        // J1196J uses the high nibble of this same count for missed ACKs.
        self.errors += 1;
        if self.errors >= 10 {
            self.down();
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
            Outcome::EchoMismatch => self.down(),
            Outcome::BadEnd => self.frame_error(),
            Outcome::ArbitrationLost | Outcome::Late => {}
        }
    }
}
