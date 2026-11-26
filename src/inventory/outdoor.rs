//! Outdoor temperature is network-wide, not an addressed thermostat field.
use super::{
    Answer,
    broadcast::{Broadcasts, Value},
};
use crate::trpc::{Call, Reply, method, service};

pub(super) const METHOD: u32 = method::OUTDOOR_TEMP;
const HOST_SAMPLE_SECONDS: u32 = 240;
const NETWORK_ONLY_EXPIRY_SECONDS: u32 = 660;
const NETWORK_WITH_FALLBACK_SECONDS: u32 = 630;
const HOST_REBROADCAST_SECONDS: u32 = 600;
const SUPPLY_LIMIT: u16 = 0x0bea; // exclusive J1196J limit for a host update
pub(super) struct Outdoor {
    network: u16,
    supplied: u16, // value + 1; zero-initialized means no host source
    heard: u32,
    updated: u32,
    broadcast: u32,
    pub(super) dirty: bool,
}

impl Outdoor {
    pub(super) const EMPTY: Self = Self {
        network: 0,
        supplied: 0,
        heard: 0,
        updated: 0,
        broadcast: 0,
        dirty: false,
    };
    pub(super) fn value(&self) -> u16 {
        if self.network == 0 {
            self.supplied.wrapping_sub(1)
        } else {
            self.network
        }
    }
    pub(super) fn supplies(&self, word: u16) -> bool {
        self.network == 0 && self.supplied.checked_sub(1) == Some(word)
    }
    pub(super) fn publish(&mut self, now: u32, queue: &mut Broadcasts) {
        queue.push(Value::Outdoor(self.value()), now);
        self.broadcast = now;
    }
    fn changed(&mut self, before: u16, now: u32, reporting: bool, queue: &mut Broadcasts) {
        if self.value() != before {
            self.dirty |= reporting;
            if self.network == 0 && self.supplied != 0 {
                self.publish(now, queue);
            }
        }
    }
    pub(super) fn observe(
        &mut self,
        data: &[u8],
        now: u32,
        reporting: bool,
        queue: &mut Broadcasts,
    ) {
        let [a, b, ..] = data else { return };
        let before = self.value();
        self.network = match u16::from_le_bytes([*a, *b]) {
            v if v == u16::MAX || super::codes::TEMP_FAULT_WORDS.contains(&v) => 0,
            word => word,
        };
        self.heard = now;
        self.dirty |= reporting;
        self.changed(before, now, reporting, queue);
    }
    pub(super) fn tick(&mut self, now: u32, reporting: bool, queue: &mut Broadcasts) {
        let before = self.value();
        if now.wrapping_sub(self.updated) >= HOST_SAMPLE_SECONDS {
            self.supplied = 0;
        }
        if now.wrapping_sub(self.heard)
            >= if self.supplied == 0 {
                NETWORK_ONLY_EXPIRY_SECONDS
            } else {
                NETWORK_WITH_FALLBACK_SECONDS
            }
        {
            self.network = 0;
        }
        self.changed(before, now, reporting, queue);
        if self.network == 0
            && self.supplied != 0
            && now.wrapping_sub(self.broadcast) >= HOST_REBROADCAST_SECONDS
        {
            self.publish(now, queue);
        }
    }
    pub(super) fn handle(
        &mut self,
        call: &Call<'_>,
        now: u32,
        reporting: bool,
        queue: &mut Broadcasts,
    ) -> Answer {
        match (call.service, call.body) {
            (service::REQUEST, [] | [_, _]) => {}
            (service::UPDATE, [a, b]) => {
                let word = u16::from_le_bytes([*a, *b]);
                if word < SUPPLY_LIMIT {
                    let before = self.value();
                    self.supplied = word + 1;
                    self.updated = now;
                    self.changed(before, now, reporting, queue);
                }
            }
            _ => return Answer::Silent,
        }
        Answer::Reply(call.ok(&self.value().to_le_bytes()))
    }
    pub(super) fn report(&mut self) -> Option<Reply> {
        core::mem::take(&mut self.dirty).then(|| Reply::report(METHOD, &self.value().to_le_bytes()))
    }
}
