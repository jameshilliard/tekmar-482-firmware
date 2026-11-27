//! Boot-lifetime tN4 link traffic. Reporting and serial queues never gate accounting.
use super::pack;
use crate::tn4::{self, HEADER_BYTES, LAST_PORT, NODE_MASK, PORT_COUNT, message, tx::Outcome};
use crate::trpc::{BridgeError, Call, Reply};

const CAPACITY: usize = 256;
const BUCKETS: usize = 256;
const COUNT_BYTES: usize = 4 * size_of::<u64>();
type Path = [u8; 3];

#[derive(Clone, Copy, Default)]
struct Counts {
    rx: u64,
    ack: u64,
    no_ack: u64,
    failed: u64,
}

impl Counts {
    const ZERO: Self = Self {
        rx: 0,
        ack: 0,
        no_ack: 0,
        failed: 0,
    };
    fn tx(&mut self, outcome: Outcome) {
        let n = match outcome {
            Outcome::Ack => &mut self.ack,
            Outcome::NoAck => &mut self.no_ack,
            _ => &mut self.failed,
        };
        *n = n.saturating_add(1);
    }
    fn wire(self) -> [u8; COUNT_BYTES] {
        pack(&[[self.rx, self.ack, self.no_ack, self.failed]
            .map(u64::to_le_bytes)
            .as_flattened()])
    }
}

#[derive(Clone, Copy)]
struct Entry {
    path: Path,
    next: u16, // One-based index; zero ends the bucket chain.
    counts: Counts,
}

#[derive(Clone, Copy)]
struct Port {
    counts: Counts,
    invalid: u64,
    rx_untracked: u64,
    tx_unattributed: u64,
}

pub struct Traffic {
    ports: [Port; PORT_COUNT],
    keys: [Entry; CAPACITY],
    heads: [u16; BUCKETS],
    used: u16,
}

impl Traffic {
    pub const EMPTY: Self = Self {
        ports: [Port {
            counts: Counts::ZERO,
            invalid: 0,
            rx_untracked: 0,
            tx_unattributed: 0,
        }; PORT_COUNT],
        keys: [Entry {
            path: [0; 3],
            next: 0,
            counts: Counts::ZERO,
        }; CAPACITY],
        heads: [0; BUCKETS],
        used: 0,
    };

    fn bucket([port, subnet, node]: Path) -> usize {
        usize::from(port.wrapping_mul(97) ^ subnet.rotate_left(3) ^ node.wrapping_mul(17))
    }
    fn index(&self, path: Path) -> Option<usize> {
        let mut link = self.heads[Self::bucket(path)];
        while link != 0 {
            let i = usize::from(link - 1);
            let entry = &self.keys[i];
            if entry.path == path {
                return Some(i);
            }
            link = entry.next;
        }
        None
    }
    fn entry(&mut self, path: Path) -> Option<&mut Counts> {
        let i = if let Some(i) = self.index(path) {
            i
        } else {
            let i = usize::from(self.used);
            if i == CAPACITY {
                return None;
            }
            let head = &mut self.heads[Self::bucket(path)];
            self.keys[i].path = path;
            self.keys[i].next = *head;
            self.used += 1;
            *head = self.used;
            i
        };
        Some(&mut self.keys[i].counts)
    }
    pub fn received(&mut self, port: usize, body: &[u8]) {
        let counts = &mut self.ports[port].counts;
        counts.rx = counts.rx.saturating_add(1);
        let path = header(body).map(|h| {
            [
                port as u8,
                (h >> tn4::SUBNET_SHIFT) as u8,
                (h >> tn4::NODE_SHIFT) as u8 & NODE_MASK,
            ]
        });
        if let Some(counts) = path.and_then(|p| self.entry(p)) {
            counts.rx = counts.rx.saturating_add(1);
        } else {
            let n = &mut self.ports[port].rx_untracked;
            *n = n.saturating_add(1);
        }
    }
    pub fn invalid(&mut self, port: usize) {
        let n = &mut self.ports[port].invalid;
        *n = n.saturating_add(1);
    }
    pub fn transmitted(&mut self, port: usize, body: &[u8], outcome: Outcome) {
        self.ports[port].counts.tx(outcome);
        if let Some(counts) = destination(body).and_then(|[s, n]| self.entry([port as u8, s, n])) {
            counts.tx(outcome);
        } else {
            let n = &mut self.ports[port].tx_unattributed;
            *n = n.saturating_add(1);
        }
    }

    pub fn read(&self, call: &Call<'_>) -> Reply {
        match call.body {
            [] => call.ok(&pack::<4>(&[
                &(CAPACITY as u16).to_le_bytes(),
                &self.used.to_le_bytes(),
            ])),
            [p @ 0..=LAST_PORT] => {
                let port = &self.ports[usize::from(*p)];
                call.ok(&pack::<57>(&[
                    &[*p],
                    &port.counts.wire(),
                    &port.invalid.to_le_bytes(),
                    &port.rx_untracked.to_le_bytes(),
                    &port.tx_unattributed.to_le_bytes(),
                ]))
            }
            [lo, hi] => {
                let index = u16::from_le_bytes([*lo, *hi]);
                if index >= self.used {
                    return call.error(BridgeError::InvalidRequest);
                }
                let e = &self.keys[usize::from(index)];
                call.ok(&pack::<37>(&[call.body, &e.path, &e.counts.wire()]))
            }
            &[port @ 0..=LAST_PORT, subnet, node @ 0..=NODE_MASK] => {
                let counts = self
                    .index([port, subnet, node])
                    .map(|i| self.keys[i].counts);
                call.ok(&pack::<36>(&[
                    call.body,
                    &[u8::from(counts.is_some())],
                    &counts.unwrap_or_default().wire(),
                ]))
            }
            _ => call.error(BridgeError::InvalidRequest),
        }
    }
}

fn header(body: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(
        body.get(..HEADER_BYTES)?.try_into().ok()?,
    ))
}
fn destination(body: &[u8]) -> Option<[u8; 2]> {
    match (
        (header(body)? >> tn4::MESSAGE_SHIFT) & tn4::MESSAGE_MASK,
        &body[HEADER_BYTES..],
    ) {
        (message::BLOCK_REQUEST, [_, _, subnet, a, b, c, d, ..]) => {
            let mask = u32::from_le_bytes([*a, *b, *c, *d]);
            (mask.is_power_of_two() && mask.leading_zeros() > 0)
                .then(|| [*subnet, mask.trailing_zeros() as u8 + 1])
        }
        (message::BLOCK_UPDATE, [subnet, node, _, _, ..]) if node & NODE_MASK != 0 => {
            Some([*subnet, *node & NODE_MASK])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sources_targets_and_unaddressed_frames_stay_distinct() {
        let mut t = Traffic::EMPTY;
        let rx = [0x76, 0xf4, 0x21, 0xc0]; // subnet 0x42, node 7
        let request = [0x6d, 0x70, 0, 7, 0, 0, 0x42, 0x40, 0, 0, 0];
        let update = [0x6d, 0xf0, 0, 4, 0x42, 7, 5, 0];
        t.received(3, &rx);
        t.invalid(3);
        t.transmitted(3, &request, Outcome::Ack);
        t.transmitted(3, &update, Outcome::NoAck);
        t.transmitted(3, &request, Outcome::ArbitrationLost);
        t.transmitted(3, &rx, Outcome::Ack);
        let p = &t.ports[3];
        assert_eq!(
            (
                p.counts.rx,
                p.invalid,
                p.counts.ack,
                p.counts.no_ack,
                p.counts.failed,
                p.tx_unattributed
            ),
            (1, 1, 2, 1, 1, 1)
        );
        let c = &t.keys[t.index([3, 0x42, 7]).unwrap()].counts;
        assert_eq!((c.rx, c.ack, c.no_ack, c.failed), (1, 1, 1, 1));
        assert_eq!(t.used, 1);
    }
    #[test]
    fn full_registry_keeps_existing_counts_and_never_wraps() {
        let mut t = Traffic::EMPTY;
        for subnet in 0..=u8::MAX {
            assert!(t.entry([0, subnet, 1]).is_some());
        }
        assert!(t.entry([1, 0, 1]).is_none());
        t.received(1, &[0, 0, 0, 0]);
        assert_eq!(t.ports[1].rx_untracked, 1);
        t.entry([0, 0, 1]).unwrap().rx = u64::MAX;
        t.received(0, &[0, 0, 0, 64]);
        assert_eq!(t.entry([0, 0, 1]).unwrap().rx, u64::MAX);
        let entry = t.entry([0, 255, 1]).unwrap();
        entry.ack = u64::MAX;
        entry.tx(Outcome::Ack);
        assert_eq!(&entry.wire()[8..16], &[255; 8]);
    }
}
