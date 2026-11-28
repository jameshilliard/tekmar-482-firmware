//! Metadata specific to the Normal-mode trace.
pub const TX: u8 = 1;
pub const GAP: u8 = 5;
pub const SETTING: u8 = 0;
pub const BROADCAST: u8 = 1;
pub const DISCOVERY: u8 = 2;
pub const PROBE: u8 = 3;
pub const STAGE_COMMAND: u8 = 4;
pub const SERVICE_FRAME: u8 = 5;

/// Consecutive unqueued observations, retired only when their marker can be enqueued.
#[derive(Default)]
pub struct PendingGap {
    ports: u8,
    count: u64,
}

impl PendingGap {
    pub const EMPTY: Self = Self { ports: 0, count: 0 };

    pub fn lost(&mut self, port: usize) {
        self.ports |= 1 << port;
        self.count = self.count.saturating_add(1);
    }

    pub fn pending(&self) -> bool {
        self.ports != 0
    }

    pub fn take(&mut self) -> Option<[u8; 2 + size_of::<u64>()]> {
        if !self.pending() {
            return None;
        }
        let mut body = [0; 2 + size_of::<u64>()];
        body[..2].copy_from_slice(&[GAP, self.ports]);
        body[2..].copy_from_slice(&self.count.to_le_bytes());
        *self = Self::EMPTY;
        Some(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loss_counts_records_not_ports_and_saturates() {
        let mut gap = PendingGap::EMPTY;
        for port in [0, 4, 0] {
            gap.lost(port);
        }
        assert_eq!(gap.take(), Some([5, 17, 3, 0, 0, 0, 0, 0, 0, 0]));
        assert!(gap.take().is_none());
        gap.count = u64::MAX;
        gap.lost(2);
        assert_eq!(
            gap.take(),
            Some([5, 4, 255, 255, 255, 255, 255, 255, 255, 255])
        );
    }
}
