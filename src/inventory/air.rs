//! Air-group master/member observations. The master's inhibit code is gateway-wide.

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
}

pub(super) struct AirGroups([u8; 17]);

impl AirGroups {
    pub const EMPTY: Self = Self([0; 17]);

    pub fn observe(&mut self, message: u32, data: &[u8]) -> Option<AirStatus> {
        let (&group, rest) = data.split_first()?;
        let index = usize::from(group & 31);
        let (heat, cool, setback, temperature, cool_capable) = match (message, rest) {
            (0x36f, &[flags, target, lo, hi, _, _, _, ..]) => {
                let side = (flags, target);
                let cool = group & 32 != 0;
                (
                    if cool { (0, 0) } else { side },
                    if cool { side } else { (0, 0) },
                    flags,
                    [lo, hi],
                    cool && flags & 1 != 0,
                )
            }
            (0x377, &[_, h, ht, lo, hi, c, ct, _, _, _, _, _, ..]) => {
                if let Some(mode) = self.0.get_mut(index) {
                    *mode = (group >> 5) & 3;
                }
                ((h, ht), (c, ct), h, [lo, hi], true)
            }
            _ => return None,
        };
        let inhibit = self.0.get(index).copied().unwrap_or(0);
        let calling_heat = heat.0 & 3 == 3;
        let calling_cool = cool.0 & 3 == 3;
        // When only one side is enabled it supplies the target even without demand.
        // When both are enabled and idle neither does. Heat wins simultaneous calls.
        let selected = if heat.0 & 1 != 0 && (cool.0 & 1 == 0 || calling_heat) {
            Some((Side::Heat, heat, 1))
        } else if cool.0 & 1 != 0 && (heat.0 & 1 == 0 || calling_cool) {
            Some((Side::Cool, cool, 2))
        } else {
            None
        };
        Some(AirStatus {
            temperature,
            setback: (setback >> 2) & 7,
            demand: if calling_heat {
                1
            } else if calling_cool {
                3
            } else {
                0
            },
            cool_capable,
            target: selected.and_then(|(side, (flags, value), denied)| {
                (inhibit != denied && flags & 0xc0 == 0 && value != 0).then_some((side, value))
            }),
        })
    }
}
