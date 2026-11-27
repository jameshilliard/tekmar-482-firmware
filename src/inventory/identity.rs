//! Masked block-zero fields and documented legacy identification quirks.
use super::{UNKNOWN, block_byte};

const HAS_PRODUCT: u8 = 1;
const HAS_J: u8 = 2;
const HAS_Z: u8 = 4;
const VERSION_FIELDS: u8 = HAS_J | HAS_Z;
const ALL: u8 = HAS_PRODUCT | VERSION_FIELDS;
const Z_RADIX: u32 = 100;

#[derive(Clone, Copy, Default)]
pub(super) struct Identity {
    product: u32,
    j: u16,
    z: u8,
    known: u8,
}

impl Identity {
    pub(super) const EMPTY: Self = Self {
        product: 0,
        j: 0,
        z: 0,
        known: 0,
    };

    pub(super) fn read(mask: u8, values: &[u8]) -> Option<Self> {
        let get = |i| block_byte(mask, values, i);
        let mut id = Self::EMPTY;
        if let (Some(lo), Some(mid), Some(hi)) = (get(0), get(1), get(2)) {
            id.product = u32::from_le_bytes([lo, mid, hi, 0]);
            id.known |= HAS_PRODUCT;
        }
        if let (Some(lo), Some(hi)) = (get(3), get(4)) {
            id.j = u16::from_le_bytes([lo, hi]);
            if id.j == 0 {
                return None; // The 483 rejects the entire block, not just the J number.
            }
            id.known |= HAS_J;
        }
        if let Some(z) = get(5) {
            id.z = z;
            id.known |= HAS_Z;
        }
        Some(id)
    }

    pub(super) fn conflicts(self, other: Self) -> bool {
        let both = self.known & other.known;
        both & HAS_PRODUCT != 0 && self.product != other.product
            || both & HAS_J != 0 && self.j != other.j
            || both & HAS_Z != 0 && self.z != other.z
    }

    pub(super) fn merge(&mut self, partial: Self) {
        if partial.known & HAS_PRODUCT != 0 {
            self.product = partial.product;
        }
        if partial.known & HAS_J != 0 {
            self.j = partial.j;
        }
        if partial.known & HAS_Z != 0 {
            self.z = partial.z;
        }
        self.known |= partial.known;
    }

    pub(super) fn complete(self) -> bool {
        self.known == ALL
    }

    pub(super) fn product(self) -> u32 {
        if self.known & HAS_PRODUCT == 0 {
            return UNKNOWN;
        }
        // The 537 exception requires an explicitly reported zero, not a missing product.
        if self.product == 0 && self.j == 1161 {
            100102
        } else {
            self.product
        }
    }

    pub(super) fn version(self) -> u32 {
        if self.known & VERSION_FIELDS != VERSION_FIELDS {
            return UNKNOWN;
        }
        // SW 11 Block.fixup_device_identification, limited to documented product/J pairs.
        let j = match (self.product(), self.j.swap_bytes()) {
            (99301 | 15041281 | 10599181, 1118) => 1118, // 541
            (99302 | 15106817 | 10599181, 1116) => 1116, // 542
            (101101 | 3006346, 1141) => 1141,            // 161
            (101102 | 3071882, 1142) => 1142,            // 162
            _ => self.j,
        };
        u32::from(j) * Z_RADIX + u32::from(self.z)
    }
}
