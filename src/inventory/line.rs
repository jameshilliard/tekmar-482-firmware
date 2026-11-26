//! Per-line Normal-mode discovery, fault accounting and transmit backoff.
use crate::tn4::{
    message,
    tx::{Outcome, Request},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineProbe {
    MasterQuery,
    NetworkInit,
    Identity { subnet: u8, node: u8 },
}

impl LineProbe {
    pub fn frame(self, port: u8) -> Option<Request> {
        match self {
            Self::MasterQuery => Request::gateway(port, message::MASTER_QUERY, &[]),
            Self::NetworkInit => Request::gateway(port, message::NETWORK_INIT_REQUEST, &[]),
            Self::Identity {
                subnet,
                node: node @ 1..=31,
            } => Request::new(
                port,
                &super::block_request(super::block::DEVICE_INFO, subnet, usize::from(node - 1)),
            ),
            Self::Identity { .. } => None,
        }
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
    identity_node: u8,
    identity_subnet: u8,
    errors: u8,
    init: Init,
    backoff_us: u32,
    idle_since: Option<u32>,
}

// Stock 0x57e4 decrements 0x643 times in idle sample phase only.
// The timer match is 0x27 ticks at 16 us each (0x7f24).
const IDLE_MATCHES: u32 = 1_603;
const IDLE_MATCH_TICKS: u32 = 39;
const TICK_US: u32 = 16;
const BACKOFF_US: u32 = IDLE_MATCHES * IDLE_MATCH_TICKS * TICK_US;
const INIT_FOLLOWUP_SECONDS: u32 = 30;
const FRAME_ERROR_THRESHOLD: u8 = 10;
const NO_ACK_WEIGHT: u8 = 1 << 4;
const NO_ACK_THRESHOLD: u8 = 2 * NO_ACK_WEIGHT;

impl NormalLine {
    pub(super) const COLD: Self = Self {
        up: false,
        sampled_up: false,
        query: false,
        identity_node: 0,
        identity_subnet: 0,
        errors: 0,
        init: Init::Cold,
        backoff_us: 0,
        idle_since: None,
    };

    pub(super) fn peer(&mut self, message: u32) {
        self.up = true;
        self.errors = 0;
        if message == message::NETWORK_INIT_REQUEST {
            self.init = Init::Done;
        }
    }

    pub(super) fn tick(&mut self, now: u32) {
        if self.up && !self.sampled_up {
            self.query = true;
            self.init = Init::AfterLinkUp(now);
        } else if matches!(self.init, Init::Cold)
            || matches!(self.init, Init::AfterLinkUp(since) if now.wrapping_sub(since) >= INIT_FOLLOWUP_SECONDS)
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
            (self.identity_node != 0).then_some(LineProbe::Identity {
                subnet: self.identity_subnet,
                node: self.identity_node,
            })
        }
    }

    pub fn identify(&mut self, subnet: u8, node: u8) -> bool {
        if self.identity_node != 0 || !(1..=31).contains(&node) {
            return false;
        }
        self.identity_subnet = subnet;
        self.identity_node = node;
        true
    }

    /// Arbitration can return this probe to the queue; retire it only on a terminal result.
    pub fn finished(&mut self, probe: LineProbe) {
        if matches!(probe, LineProbe::Identity { subnet, node } if subnet == self.identity_subnet && node == self.identity_node)
        {
            self.identity_node = 0;
            return;
        }
        if self.pending() != Some(probe) {
            return; // A peer or a newer link-up already changed the pending action.
        }
        match probe {
            LineProbe::MasterQuery => self.query = false,
            LineProbe::NetworkInit => self.init = Init::Done,
            LineProbe::Identity { .. } => {}
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
        if self.errors >= FRAME_ERROR_THRESHOLD {
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
                self.errors += NO_ACK_WEIGHT;
                if self.errors >= NO_ACK_THRESHOLD {
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
