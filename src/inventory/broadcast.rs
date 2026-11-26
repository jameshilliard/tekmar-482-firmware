//! Normal-mode broadcasts share one ordered queue, with one copy per line.
use crate::tn4::tx::Request;

#[derive(Clone, Copy)]
pub(super) enum Value {
    Group(u8, u8),
    Outdoor(u16),
    RequestedOutdoor(u16),
}

impl Value {
    fn key(self) -> u8 {
        match self {
            Self::Group(id, _) => id, // valid group IDs start at one
            Self::Outdoor(_) | Self::RequestedOutdoor(_) => 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Broadcast {
    value: Value,
    since: u32,
}

impl Broadcast {
    pub fn frame(self, port: u8) -> Option<Request> {
        match self.value {
            Value::Group(id, value) => Request::new(port, &[0x61, 0xf0, 0, 2, id, value]),
            Value::Outdoor(word) | Value::RequestedOutdoor(word) => {
                let [lo, hi] = word.to_le_bytes();
                Request::new(port, &[0x72, 0xf0, 0, 4, lo, hi, lo, hi])
            }
        }
    }

    pub fn outdoor_echo(self, frame: &[u8]) -> bool {
        matches!(self.value, Value::Outdoor(word) | Value::RequestedOutdoor(word) if matches!(frame,
            [0x72, 0xf0, 0, 4, a, b, c, d] if [*a, *b] == word.to_le_bytes() && a == c && b == d))
    }
}

#[derive(Clone, Copy)]
struct Queued {
    send: Broadcast,
    ports: u8,
}

const CAPACITY: usize = 13; // twelve demand groups plus outdoor temperature
const TTL: u32 = 10;

pub(super) struct Broadcasts {
    queue: [Queued; CAPACITY],
    len: usize,
}

impl Broadcasts {
    pub(super) const EMPTY: Self = Self {
        queue: [Queued {
            send: Broadcast {
                value: Value::Group(0, 0),
                since: 0,
            },
            ports: 0,
        }; CAPACITY],
        len: 0,
    };
    pub(super) fn push(&mut self, value: Value, now: u32) {
        if let Some(i) = self.queue[..self.len]
            .iter()
            .position(|q| q.send.value.key() == value.key())
        {
            self.remove(i);
        }
        self.queue[self.len] = Queued {
            send: Broadcast { value, since: now },
            ports: (1 << super::PORTS) - 1,
        };
        self.len += 1;
    }
    fn remove(&mut self, i: usize) {
        self.len -= 1;
        for j in i..self.len {
            self.queue.swap(j, j + 1);
        }
    }
    pub(super) fn pending(&self, port: usize) -> bool {
        self.queue[..self.len]
            .iter()
            .any(|q| q.ports & (1 << port) != 0)
    }
    pub(super) fn stale(&self, port: usize, tx: Broadcast, now: u32) -> bool {
        now.wrapping_sub(tx.since) >= TTL
            || self.queue[..self.len]
                .iter()
                .any(|q| q.send.value.key() == tx.value.key() && q.ports & (1 << port) != 0)
    }
    pub(super) fn take(&mut self, port: usize, now: u32) -> Option<Broadcast> {
        let mut i = 0;
        while i < self.len {
            let q = self.queue[i];
            if now.wrapping_sub(q.send.since) >= TTL {
                self.remove(i);
            } else if q.ports & (1 << port) != 0 {
                self.queue[i].ports &= !(1 << port);
                if self.queue[i].ports == 0 {
                    self.remove(i);
                }
                return Some(q.send);
            } else {
                i += 1;
            }
        }
        None
    }

    pub(super) fn stale_outdoor(tx: Broadcast, source: &super::outdoor::Outdoor) -> bool {
        matches!(tx.value, Value::Outdoor(word) if !source.supplies(word))
    }
}
