//! Air-group master/member observations. The master's inhibit code is gateway-wide.
use super::codes::{demand, setback};
use crate::tn4::message;
const INDEXED_GROUPS: usize = 17;
const GROUP_MASK: u8 = 0x1f;
const GROUP_MASTER: u8 = 1 << 7;
const ROLE_MASK: u8 = GROUP_MASK | GROUP_MASTER;
const MEMBER_COOL: u8 = 1 << 5;
const MASTER_INHIBIT_SHIFT: u8 = 5;
const MASTER_INHIBIT_MASK: u8 = 3;
const INHIBIT_HEAT: u8 = 1;
const INHIBIT_COOL: u8 = 2;
const ENABLED: u8 = 1;
const DEMAND: u8 = 1 << 1;
const CALLING: u8 = ENABLED | DEMAND;
const SETBACK_SHIFT: u8 = 2;
const OPTIMUM_START_HIGH: u8 = 1 << 6;
const WEATHER_SHUTDOWN: u8 = 1 << 7;
// Local cache bits above the five-bit group number, not tN4 status flags.
const HEAT_ENABLED: u8 = 1 << 5;
const COOL_ENABLED: u8 = 1 << 6;

/// Pack the last role and mode confirmation into one byte per device.
#[derive(Clone, Copy, Default)]
pub(super) struct ModeEvidence(u8);

impl ModeEvidence {
    pub const EMPTY: Self = Self(0);
    // These occupy bits outside ROLE_MASK and are not copied from the wire.
    const ROLE_OBSERVED: u8 = 1 << 5;
    const MODE_CONFIRMED: u8 = 1 << 6;

    pub fn confirmed(self) -> bool {
        self.0 & Self::MODE_CONFIRMED != 0
    }

    pub fn set_confirmed(&mut self, confirmed: bool) {
        if confirmed {
            self.0 |= Self::MODE_CONFIRMED;
        } else {
            self.0 &= !Self::MODE_CONFIRMED;
        }
    }

    pub fn role_changed(&mut self, data: &[u8]) -> bool {
        let [role, previous, ..] = data else {
            return false;
        };
        let prior = if self.0 & Self::ROLE_OBSERVED != 0 {
            self.0
        } else {
            *previous
        } & ROLE_MASK;
        let current = role & ROLE_MASK;
        self.0 = (self.0 & Self::MODE_CONFIRMED) | Self::ROLE_OBSERVED | current;
        prior != current
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Heat,
    Cool,
}

pub(super) struct AirStatus {
    pub temperature: [u8; 2],
    pub setback: u8,
    pub demand: u8,
    pub cool_capable: bool,
    pub target: Option<(Side, u8)>,
    group: u8,
    observed_sides: u8,
    enabled_sides: u8,
}

impl AirStatus {
    pub fn mode_hint(&self, previous: u8) -> u8 {
        // A member reports one side; absence of the other side is not "disabled".
        let keep = if previous & GROUP_MASK == self.group {
            previous & (HEAT_ENABLED | COOL_ENABLED) & !self.observed_sides
        } else {
            0
        };
        self.group | self.enabled_sides | keep
    }
}

pub(super) struct AirGroups([u8; INDEXED_GROUPS]);

impl AirGroups {
    pub const EMPTY: Self = Self([0; INDEXED_GROUPS]);

    pub fn observe(&mut self, message: u32, data: &[u8]) -> Option<AirStatus> {
        let (&group, rest) = data.split_first()?;
        let index = usize::from(group & GROUP_MASK);
        let (heat, cool, setback, temperature, cool_capable) = match (message, rest) {
            (message::AIR_GROUP_MEMBER_STATUS, &[flags, target, lo, hi, _, _, _, ..]) => {
                let side = (flags, target);
                let cool = group & MEMBER_COOL != 0;
                (
                    if cool { (0, 0) } else { side },
                    if cool { side } else { (0, 0) },
                    flags,
                    [lo, hi],
                    cool && flags & ENABLED != 0,
                )
            }
            (message::AIR_GROUP_MASTER_STATUS, &[_, h, ht, lo, hi, c, ct, _, _, _, _, _, ..]) => {
                if let Some(mode) = self.0.get_mut(index) {
                    *mode = (group >> MASTER_INHIBIT_SHIFT) & MASTER_INHIBIT_MASK;
                }
                ((h, ht), (c, ct), h, [lo, hi], true)
            }
            _ => return None,
        };
        let inhibit = self.0.get(index).copied().unwrap_or(0);
        let calling_heat = heat.0 & CALLING == CALLING;
        let calling_cool = cool.0 & CALLING == CALLING;
        // When only one side is enabled it supplies the target even without demand.
        // When both are enabled and idle neither does. Heat wins simultaneous calls.
        let selected = if heat.0 & ENABLED != 0 && (cool.0 & ENABLED == 0 || calling_heat) {
            Some((Side::Heat, heat, INHIBIT_HEAT))
        } else if cool.0 & ENABLED != 0 && (heat.0 & ENABLED == 0 || calling_cool) {
            Some((Side::Cool, cool, INHIBIT_COOL))
        } else {
            None
        };
        Some(AirStatus {
            temperature,
            group: group & GROUP_MASK,
            observed_sides: if message == message::AIR_GROUP_MASTER_STATUS {
                HEAT_ENABLED | COOL_ENABLED
            } else if group & MEMBER_COOL != 0 {
                COOL_ENABLED
            } else {
                HEAT_ENABLED
            },
            enabled_sides: (u8::from(heat.0 & ENABLED != 0) * HEAT_ENABLED)
                | (u8::from(cool.0 & ENABLED != 0) * COOL_ENABLED),
            setback: (setback >> SETBACK_SHIFT) & setback::MASK,
            demand: if calling_heat {
                demand::HEAT
            } else if calling_cool {
                demand::COOL
            } else {
                demand::NONE
            },
            cool_capable,
            target: selected.and_then(|(side, (flags, value), denied)| {
                (inhibit != denied
                    && flags & (OPTIMUM_START_HIGH | WEATHER_SHUTDOWN) == 0
                    && value != 0)
                    .then_some((side, value))
            }),
        })
    }
}
