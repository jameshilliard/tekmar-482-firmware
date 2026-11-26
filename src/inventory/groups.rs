//! Global demand groups are independent of port inventories and device addresses.
use super::Answer;
use crate::{
    tn4::tx::Request,
    trpc::{Call, Reply},
};

pub(super) const METHOD: u32 = 0x13d;
const COUNT: usize = 12;
const ALL_PORTS: u8 = (1 << super::PORTS) - 1;
const ACTIVE_SECONDS: u32 = 60;
const SEND_SECONDS: u32 = 10;

#[derive(Clone, Copy, Default)]
struct Queued {
    group: u8,
    value: u8,
    ports: u8,
    since: u32,
}

#[derive(Clone, Copy)]
pub struct GroupSend {
    group: u8,
    value: u8,
    since: u32,
}

impl GroupSend {
    pub fn frame(self, port: u8) -> Option<Request> {
        Request::new(port, &[0x61, 0xf0, 0, 2, self.group, self.value])
    }
}

pub(super) struct Groups {
    active: u16,
    dirty: u16,
    seen: [u32; COUNT],
    queue: [Queued; COUNT],
    len: usize,
}

impl Groups {
    pub(super) const EMPTY: Self = Self {
        active: 0,
        dirty: 0,
        seen: [0; COUNT],
        queue: [Queued {
            group: 0,
            value: 0,
            ports: 0,
            since: 0,
        }; COUNT],
        len: 0,
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
    pub(super) fn handle(&mut self, call: &Call<'_>, now: u32) -> Answer {
        match (call.service, call.body) {
            (1, [id] | [id, _]) => {
                let value = Self::index(*id).map_or(255, |i| self.value(i, now));
                Answer::Reply(call.ok(&[*id, value]))
            }
            (0, [id, value]) => {
                if Self::index(*id).is_none() || *value > 1 {
                    return Answer::Reply(call.ok(&[*id, 255]));
                }
                if let Some(i) = self.queue[..self.len].iter().position(|q| q.group == *id) {
                    self.remove(i);
                }
                self.queue[self.len] = Queued {
                    group: *id,
                    value: *value,
                    ports: ALL_PORTS,
                    since: now,
                };
                self.len += 1;
                // Do not publish a buffered pre-command observation as a new report.
                self.dirty &= !(1 << (*id - 1));
                Answer::Silent
            }
            _ => Answer::Silent,
        }
    }
    fn remove(&mut self, i: usize) {
        self.queue.copy_within(i + 1..self.len, i);
        self.len -= 1;
    }
    pub(super) fn pending(&self, p: usize) -> bool {
        self.queue[..self.len]
            .iter()
            .any(|q| q.ports & (1 << p) != 0)
    }
    pub(super) fn stale(&self, p: usize, tx: GroupSend, now: u32) -> bool {
        now.wrapping_sub(tx.since) >= SEND_SECONDS
            || self.queue[..self.len]
                .iter()
                .any(|q| q.group == tx.group && q.ports & (1 << p) != 0)
    }
    pub(super) fn take(&mut self, p: usize, now: u32) -> Option<GroupSend> {
        let mut i = 0;
        while i < self.len {
            let q = self.queue[i];
            if now.wrapping_sub(q.since) >= SEND_SECONDS {
                self.remove(i);
            } else if q.ports & (1 << p) != 0 {
                self.queue[i].ports &= !(1 << p);
                if self.queue[i].ports == 0 {
                    self.remove(i);
                }
                return Some(GroupSend {
                    group: q.group,
                    value: q.value,
                    since: q.since,
                });
            } else {
                i += 1;
            }
        }
        None
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
