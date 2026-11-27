//! Configured snowmelt endpoint and status pairs, independent of the header source.

pub(crate) const REQUEST: u32 = 0x17f;
pub(crate) const REPORT: u32 = 0x17e;
const ROLE_MASK: u8 = 7;
const NUMBER_STEP: u8 = ROLE_MASK + 1;
const ZONE: u8 = 1;
const SENSOR: u8 = 2;
const MAX_ZONES: u8 = 12;
const MAX_SENSORS: u8 = 24;
const PAIR_BYTES: usize = 3;

fn limit(address: u8) -> Option<u8> {
    match address & ROLE_MASK {
        ZONE => Some(MAX_ZONES),
        SENSOR => Some(MAX_SENSORS),
        _ => None,
    }
}

pub(crate) fn report_address(destination: u8) -> Option<u8> {
    // Request destinations are zero-based; report sources are one-based.
    if destination / NUMBER_STEP >= limit(destination)? {
        return None;
    }
    Some(destination + NUMBER_STEP)
}

pub(crate) fn status(data: &[u8]) -> Option<(u8, &[[u8; PAIR_BYTES]])> {
    let &[address, count, ref rest @ ..] = data else {
        return None;
    };
    if !(1..=limit(address)?).contains(&(address / NUMBER_STEP)) {
        return None;
    }
    let pairs = rest.get(..usize::from(count) * PAIR_BYTES)?;
    Some((address, pairs.as_chunks().0))
}
