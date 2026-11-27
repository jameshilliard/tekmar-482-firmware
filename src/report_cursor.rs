//! Round-robin selection over banks of pending report bits.

pub(crate) struct ReportCursor {
    bank: usize,
    next_bit: u32,
}

impl ReportCursor {
    pub(crate) const START: Self = Self {
        bank: 0,
        next_bit: 0,
    };

    pub(crate) fn select<T>(
        &mut self,
        banks: &[u32],
        mut emit: impl FnMut((usize, usize)) -> Option<T>,
    ) -> Option<T> {
        let mut bank = self.bank;
        let suffix = u32::MAX.checked_shl(self.next_bit).unwrap_or(0);
        for offset in 0..=banks.len() {
            // Visit the starting bank in two pieces, exactly once per bit.
            let range = match offset {
                0 => suffix,
                n if n == banks.len() => !suffix,
                _ => u32::MAX,
            };
            let mut pending = banks[bank] & range;
            while pending != 0 {
                let bit = pending.trailing_zeros();
                pending &= pending - 1;
                if let Some(report) = emit((bank, bit as usize)) {
                    self.bank = bank;
                    self.next_bit = bit + 1;
                    return Some(report);
                }
            }
            bank += 1;
            if bank == banks.len() {
                bank = 0;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_selection_matches_an_exhaustive_slot_walk() {
        let banks = [1 | (1 << 31), 0, 1 << 17, 1 | (1 << 28)];
        for start in 0..u32::BITS as usize * banks.len() {
            let (bank, bit) = (start / 32, start % 32);
            let mut cursor = ReportCursor {
                bank,
                next_bit: bit as u32,
            };
            let expected = (start..start + 32 * banks.len())
                .map(|slot| ((slot / 32) % banks.len(), slot % 32))
                .filter(|&(bank, bit)| banks[bank] & (1 << bit) != 0);
            let mut visited = [None; 5];
            let mut count = 0;
            assert_eq!(
                cursor.select::<()>(&banks, |key| {
                    visited[count] = Some(key);
                    count += 1;
                    None
                }),
                None
            );
            assert!(visited.into_iter().flatten().eq(expected));
            assert_eq!((cursor.bank, cursor.next_bit), (bank, bit as u32));
        }
    }

    #[test]
    fn accepting_highest_bit_wraps_across_empty_banks() {
        let banks = [1 | (1 << 31), 0, 1 << 3];
        let mut cursor = ReportCursor::START;
        for key in [(0, 0), (0, 31), (2, 3), (0, 0)] {
            assert_eq!(cursor.select(&banks, Some), Some(key));
        }
    }
}
