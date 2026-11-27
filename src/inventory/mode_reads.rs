//! Host requests wait for peer evidence, not ownership of a line's setting transaction.
use super::settings::COMMAND_TIMEOUT_SECONDS;
use super::{ADDRESS_PORT_SCALE, Answer, BridgeError, Call, NODES, Reply, method, service};

struct Wait {
    address: u16,
    incarnation: u16,
    since: u32,
    answer: Option<u8>,
    copies: u8,
}

pub(super) struct ModeReads {
    // Enough distinct pending reads for a whole thermostat line. Identical
    // outstanding requests share an entry but each receives its own reply.
    waits: heapless::Vec<Wait, NODES>,
    ready: usize,
}

fn reply(address: u16, mode: u8) -> Reply {
    let [lo, hi] = address.to_le_bytes();
    Call {
        service: service::REQUEST,
        method: method::MODE_SETTING,
        body: &[],
    }
    .ok(&[lo, hi, mode])
}

impl ModeReads {
    pub(super) const EMPTY: Self = Self {
        waits: heapless::Vec::new(),
        ready: 0,
    };

    pub(super) fn pending(&self) -> bool {
        !self.waits.is_empty()
    }

    pub(super) fn ready_for(&self, address: u16) -> bool {
        self.ready != 0
            && self
                .waits
                .iter()
                .any(|w| w.address == address && w.answer.is_some())
    }

    pub(super) fn request(
        &mut self,
        call: &Call<'_>,
        address: u16,
        incarnation: u16,
        cached: Option<u8>,
        now: u32,
        queue_read: impl FnOnce(),
    ) -> Answer {
        if let Some(last) = self.waits.iter_mut().rfind(|w| w.address == address) {
            if last.incarnation == incarnation && last.answer == cached {
                let Some(n) = last.copies.checked_add(1) else {
                    return Answer::Reply(call.error(BridgeError::Busy));
                };
                last.copies = n;
                return Answer::Silent;
            }
        } else if let Some(value) = cached {
            return Answer::Reply(reply(address, value));
        }
        if self
            .waits
            .push(Wait {
                address,
                incarnation,
                since: now,
                answer: cached,
                copies: 1,
            })
            .is_err()
        {
            return Answer::Reply(call.error(BridgeError::Busy));
        }
        if cached.is_some() {
            self.ready += 1;
        } else {
            queue_read();
        }
        Answer::Silent
    }

    fn finish(&mut self, value: u8, mut matches: impl FnMut(&Wait) -> bool) {
        for wait in &mut self.waits {
            if wait.answer.is_none() && matches(wait) {
                wait.answer = Some(value);
                self.ready += 1;
            }
        }
    }

    pub(super) fn sample(&mut self, address: u16, incarnation: u16, mode: u8) {
        self.finish(mode, |w| {
            w.address == address && w.incarnation == incarnation
        });
    }

    pub(super) fn forget(&mut self, address: u16) {
        self.finish(super::codes::UNAVAILABLE, |w| w.address == address);
    }

    pub(super) fn forget_all(&mut self) {
        self.finish(super::codes::UNAVAILABLE, |_| true);
    }

    pub(super) fn forget_port(&mut self, port: usize) {
        self.finish(super::codes::UNAVAILABLE, |w| {
            usize::from(w.address / ADDRESS_PORT_SCALE) == port
        });
    }

    pub(super) fn expire(&mut self, now: u32, mut recover: impl FnMut(u16, u16)) {
        self.finish(super::codes::UNAVAILABLE, |w| {
            if now.wrapping_sub(w.since) < COMMAND_TIMEOUT_SECONDS {
                return false;
            }
            recover(w.address, w.incarnation);
            true
        });
    }

    pub(super) fn next(&mut self) -> Option<Reply> {
        if self.ready == 0 {
            return None;
        }
        let (i, w) = self
            .waits
            .iter_mut()
            .enumerate()
            .find(|(_, w)| w.answer.is_some())?;
        let result = reply(w.address, w.answer?);
        w.copies -= 1;
        if w.copies == 0 {
            self.waits.remove(i);
            self.ready -= 1;
        }
        Some(result)
    }
}
