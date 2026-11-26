//! Global demand groups are independent of port inventories and device addresses.
use super::{
    Answer,
    broadcast::{Broadcasts, Value},
};
use crate::trpc::{Call, Reply};

pub(super) const METHOD: u32 = 0x13d;
const COUNT: usize = 12;
const ACTIVE_SECONDS: u32 = 60;
pub(super) struct Groups {
    active: u16,
    dirty: u16,
    seen: [u32; COUNT],
}

impl Groups {
    pub(super) const EMPTY: Self = Self {
        active: 0,
        dirty: 0,
        seen: [0; COUNT],
    };

    fn index(group: u8) -> Option<usize> {
        group.checked_sub(1).map(usize::from).filter(|i| *i < COUNT)
    }
    pub(super) fn observe(&mut self, data: &[u8], now: u32, reporting: bool) {
        let [id, flags, ..] = data else { return };
        let Some(i) = Self::index(*id) else { return };
        let bit = 1 << i;
        if flags & 0x10 != 0 {
            self.active |= bit;
            self.seen[i] = now;
        } else {
            self.active &= !bit;
        }
        if reporting {
            self.dirty |= bit;
        }
    }
    fn value(&mut self, i: usize, now: u32) -> u8 {
        let bit = 1 << i;
        if self.active & bit != 0 && now.wrapping_sub(self.seen[i]) >= ACTIVE_SECONDS {
            self.active &= !bit;
            // Expiry is not a device confirming a stop.
            self.dirty &= !bit;
        }
        u8::from(self.active & bit != 0)
    }
    pub(super) fn handle(&mut self, call: &Call<'_>, now: u32, queue: &mut Broadcasts) -> Answer {
        match (call.service, call.body) {
            (1, [id] | [id, _]) => {
                let value = Self::index(*id).map_or(255, |i| self.value(i, now));
                Answer::Reply(call.ok(&[*id, value]))
            }
            (0, [id, value]) => {
                if Self::index(*id).is_none() || *value > 1 {
                    return Answer::Reply(call.ok(&[*id, 255]));
                }
                queue.push(Value::Group(*id, *value), now);
                // Do not publish a buffered pre-command observation as a new report.
                self.dirty &= !(1 << (*id - 1));
                Answer::Silent
            }
            _ => Answer::Silent,
        }
    }
    pub(super) fn clear_reports(&mut self) {
        self.dirty = 0;
    }
    pub(super) fn has_reports(&self) -> bool {
        self.dirty != 0
    }
    pub(super) fn next_report(&mut self, now: u32) -> Option<Reply> {
        while self.dirty != 0 {
            let i = self.dirty.trailing_zeros() as usize;
            let value = self.value(i, now);
            if self.dirty & (1 << i) != 0 {
                self.dirty &= !(1 << i);
                return Some(Reply::report(METHOD, &[i as u8 + 1, value]));
            }
        }
        None
    }
}
