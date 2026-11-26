//! Normal-mode broadcasts share one ordered queue, with one copy per line.
use crate::tn4::tx::Request;

#[derive(Clone, Copy)]
pub(super) enum Value {
    Group(u8, u8),
    Outdoor(u16),
    RequestedOutdoor(u16),
    Clock([u8; 5]),
}

impl Value {
    pub(super) fn clock(body: &[u8]) -> Option<Self> {
        let &[
            lo,
            hi,
            month @ 1..=12,
            day @ 1..=31,
            weekday @ 1..=7,
            hour @ 0..=23,
            minute @ 0..=59,
        ] = body
        else {
            return None;
        };
        let year = u8::try_from(u16::from_le_bytes([lo, hi]).checked_sub(2000)?).ok()?;
        Some(Self::Clock([
            minute,
            weekday | hour << 3,
            month - 1,
            day - 1,
            year,
        ]))
    }

    fn replaces(self, old: Self) -> bool {
        match (self, old) {
            (Self::Group(a, _), Self::Group(b, _)) => a == b,
            (
                Self::Outdoor(_) | Self::RequestedOutdoor(_),
                Self::Outdoor(_) | Self::RequestedOutdoor(_),
            ) => true,
            _ => false,
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
            Value::Clock([minute, time, month, day, year]) => {
                Request::new(port, &[0x71, 0xf0, 0, 5, minute, time, month, day, year])
            }
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

const CAPACITY: usize = 14; // twelve demand groups, outdoor temperature and one queued clock
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
            .position(|q| value.replaces(q.send.value))
        {
            self.remove(i);
        }
        self.queue[self.len] = Queued {
            send: Broadcast { value, since: now },
            ports: (1 << super::PORTS) - 1,
        };
        self.len += 1;
    }
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
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
                .any(|q| q.send.value.replaces(tx.value) && q.ports & (1 << port) != 0)
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
