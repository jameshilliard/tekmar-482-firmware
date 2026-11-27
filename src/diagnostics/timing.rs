//! Wall-clock bounds on cooperative line service, not an instruction count.
use crate::tn4::PORT_COUNT;

const SERVICE_FIELDS: usize = 4;
const TRANSMIT_FIELDS: usize = 4;
pub const SERVICE_BYTES: usize = SERVICE_FIELDS * size_of::<u32>();
pub const TRANSMIT_BYTES: usize = 1 + TRANSMIT_FIELDS * size_of::<u32>();

struct TransmitTiming {
    poll_at: u32,
    max_poll_gap: u32,
    miss_gap: u32,
    miss_late: u32,
    miss_ms: u32,
}

impl TransmitTiming {
    const EMPTY: Self = Self {
        poll_at: 0,
        max_poll_gap: 0,
        miss_gap: 0,
        miss_late: 0,
        miss_ms: 0,
    };
}

pub struct LinkTiming {
    pass_start: u32,
    receive_end: u32,
    line_end: Option<u32>,
    max_pass: u32,
    max_receive: u32,
    max_line: u32,
    max_host: u32,
    ports: [TransmitTiming; PORT_COUNT],
}

impl LinkTiming {
    pub const EMPTY: Self = Self {
        pass_start: 0,
        receive_end: 0,
        line_end: None,
        max_pass: 0,
        max_receive: 0,
        max_line: 0,
        max_host: 0,
        ports: [const { TransmitTiming::EMPTY }; PORT_COUNT],
    };

    pub fn begin_pass(&mut self, at: u32) {
        if let Some(line_end) = self.line_end {
            self.max_pass = self.max_pass.max(at.wrapping_sub(self.pass_start));
            self.max_host = self.max_host.max(at.wrapping_sub(line_end));
        }
        self.pass_start = at;
    }

    pub fn end_receive(&mut self, at: u32) {
        self.max_receive = self.max_receive.max(at.wrapping_sub(self.pass_start));
        self.receive_end = at;
    }

    pub fn end_lines(&mut self, at: u32) {
        self.max_line = self.max_line.max(at.wrapping_sub(self.receive_end));
        self.line_end = Some(at);
    }

    pub fn start_transmit(&mut self, port: usize, at: u32) {
        self.ports[port].poll_at = at;
    }

    pub fn poll_transmit(&mut self, port: usize, at: u32, missed: Option<u32>, ms: u32) {
        let slot = &mut self.ports[port];
        let gap = at.wrapping_sub(slot.poll_at);
        slot.max_poll_gap = slot.max_poll_gap.max(gap);
        slot.poll_at = at;
        if let Some(due) = missed {
            slot.miss_gap = gap;
            slot.miss_late = at.wrapping_sub(due);
            slot.miss_ms = ms;
        }
    }

    pub fn service(&self) -> [u8; SERVICE_BYTES] {
        super::pack(&[[
            self.max_pass,
            self.max_receive,
            self.max_line,
            self.max_host,
        ]
        .map(u32::to_le_bytes)
        .as_flattened()])
    }

    pub fn transmit(&self, port: u8) -> [u8; TRANSMIT_BYTES] {
        let slot = &self.ports[usize::from(port)];
        super::pack(&[
            &[port],
            [
                slot.max_poll_gap,
                slot.miss_gap,
                slot.miss_late,
                slot.miss_ms,
            ]
            .map(u32::to_le_bytes)
            .as_flattened(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_bounds_use_wrapping_microseconds_and_ignore_the_first_boot_gap() {
        let mut timing = LinkTiming::EMPTY;
        timing.begin_pass(u32::MAX - 9);
        timing.end_receive(u32::MAX - 4);
        timing.end_lines(1);
        timing.begin_pass(50);
        timing.end_receive(58);
        timing.end_lines(61);
        timing.begin_pass(69);
        assert_eq!(
            timing.service(),
            [60, 0, 0, 0, 8, 0, 0, 0, 6, 0, 0, 0, 49, 0, 0, 0]
        );
    }

    #[test]
    fn tx_gaps_are_within_attempt_and_last_miss_survives_later_successes() {
        let mut timing = LinkTiming::EMPTY;
        timing.start_transmit(1, u32::MAX - 700);
        timing.poll_transmit(1, 0x120, Some(u32::MAX - 400), 77);
        timing.start_transmit(1, 15_000);
        timing.poll_transmit(1, 15_005, None, 81);
        assert_eq!(timing.transmit(0), [0; TRANSMIT_BYTES]);
        assert_eq!(
            timing.transmit(1),
            [1, 0xdd, 3, 0, 0, 0xdd, 3, 0, 0, 0xb1, 2, 0, 0, 77, 0, 0, 0],
        );
    }
}
