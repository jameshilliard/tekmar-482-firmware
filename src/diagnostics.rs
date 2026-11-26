//! Little-endian bridge diagnostics. Event totals are lifetime, saturating u64;
//! hardware capture registers and pin masks remain u32.
use crate::tn4::PORT_COUNT;

pub const CAPTURE_BYTES: usize =
    (2 + PORT_COUNT) * size_of::<u32>() + PORT_COUNT * size_of::<u64>();
pub const PANEL_BYTES: usize = 5 * size_of::<u32>() + 2 * size_of::<u64>();

fn pack<const N: usize>(parts: &[&[u8]]) -> [u8; N] {
    let mut bytes = [0; N];
    let mut rest = &mut bytes[..];
    for part in parts {
        let (dest, next) = rest.split_at_mut(part.len());
        dest.copy_from_slice(part);
        rest = next;
    }
    debug_assert!(rest.is_empty());
    bytes
}

pub fn port_drops(port: u8, counts: [u64; 2]) -> [u8; 1 + 2 * size_of::<u64>()] {
    pack(&[&[port], counts.map(u64::to_le_bytes).as_flattened()])
}

pub fn trace_drops(mask: u8, counts: [u64; PORT_COUNT]) -> [u8; 1 + PORT_COUNT * size_of::<u64>()] {
    pack(&[&[mask], counts.map(u64::to_le_bytes).as_flattened()])
}

pub fn panel(inputs: [u32; 2], counts: [u64; 2], outputs: [u32; 3]) -> [u8; PANEL_BYTES] {
    pack(&[
        inputs.map(u32::to_le_bytes).as_flattened(),
        counts.map(u64::to_le_bytes).as_flattened(),
        outputs.map(u32::to_le_bytes).as_flattened(),
    ])
}

pub fn capture(
    timers: [u32; 2],
    edges: [u64; PORT_COUNT],
    ticks: [u32; PORT_COUNT],
) -> [u8; CAPTURE_BYTES] {
    pack(&[
        timers.map(u32::to_le_bytes).as_flattened(),
        edges.map(u64::to_le_bytes).as_flattened(),
        ticks.map(u32::to_le_bytes).as_flattened(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_counts_preserve_high_words_and_hardware_register_boundaries() {
        const HIGH: u64 = 0xfedc_ba98_7654_3210;
        const LE: [u8; 8] = [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe];
        let drops = port_drops(4, [HIGH, u64::MAX]);
        assert_eq!(drops.len(), 17);
        assert_eq!(drops[0], 4);
        assert_eq!(drops[1..9], LE);
        assert_eq!(drops[9..], [255; 8]);
        let trace = trace_drops(31, [0, 1, 2, 3, HIGH]);
        assert_eq!(trace.len(), 41);
        assert_eq!(trace[0], 31);
        assert_eq!(trace[33..], LE);
        let p = panel([2, 1], [u64::MAX, HIGH], [17, 3, 1]);
        assert_eq!(p.len(), 36);
        assert_eq!(p[8..16], [255; 8]);
        assert_eq!(p[16..24], LE);
        assert_eq!(p[24..], [17, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0]);
        let c = capture([1, u32::MAX], [0, 1, 2, 3, HIGH], [0xfe; PORT_COUNT]);
        assert_eq!(c.len(), 68);
        assert_eq!(c[4..8], [255; 4]);
        assert_eq!(c[40..48], LE);
        assert_eq!(c[48..52], [0xfe, 0, 0, 0]);
    }
}
