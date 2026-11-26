//! Scheduled link responses share the TX pin with a sender that is waiting for idle.
use super::{BIT_US, MAX_LATENESS_US, Receiver, State};

const FLAG_US: u32 = 6 * BIT_US;

#[derive(Default)]
pub struct LinkResponse {
    flag: Option<u32>,
    release: Option<u32>,
    handled_ack: Option<u32>,
}

impl LinkResponse {
    pub fn busy(&self) -> bool {
        self.flag.is_some() || self.release.is_some()
    }

    /// Normal mode only. Ignore our flag and consume the delimiter before receiving again.
    pub fn recover(&mut self, rx: &mut Receiver, next_bit: u32) {
        if !rx.recovering() {
            rx.state = State::ErrorFlag(next_bit.wrapping_add(FLAG_US));
            rx.verify_drive = false;
            self.flag = Some(next_bit);
        }
    }

    /// Return only physical output transitions, never start a pulse a bit late.
    pub fn poll(&mut self, now: u32, ack_slot: Option<u32>) -> Option<bool> {
        if let Some(end) = self.release {
            if (now.wrapping_sub(end) as i32) < 0 {
                return None;
            }
            self.release = None;
            return Some(false);
        }
        if ack_slot.is_none() {
            self.handled_ack = None;
        }
        let (at, width) = match (self.flag, ack_slot) {
            (Some(at), _) => (at, FLAG_US),
            (None, Some(at)) if self.handled_ack != Some(at) => (at, BIT_US),
            _ => return None,
        };
        let elapsed = now.wrapping_sub(at) as i32;
        if elapsed < 0 {
            return None;
        }
        if self.flag.take().is_none() {
            self.handled_ack = Some(at);
        }
        if elapsed >= MAX_LATENESS_US {
            return None;
        }
        self.release = Some(at.wrapping_add(width));
        Some(true)
    }
}
