//! A host operation owns one port until its device reply has been queued.
//! All writes start from a fresh block, never from the temperature target.
use super::{Answer, block_byte};
use crate::{
    tn4::tx::Request,
    trpc::{Call, Reply},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Setting {
    Mode,
    Heat,
    Cool,
    Slab,
    Fan,
    HumidityMin,
    HumidityMax,
}

impl Setting {
    pub(super) fn from_method(m: u32) -> Option<Self> {
        Some(match m {
            0x127 => Self::Mode,
            0x13f => Self::Heat,
            0x147 => Self::Cool,
            0x14f => Self::Slab,
            0x157 => Self::Fan,
            0x152 => Self::HumidityMin,
            0x151 => Self::HumidityMax,
            _ => return None,
        })
    }
    fn method(self) -> u32 {
        match self {
            Self::Mode => 0x127,
            Self::Heat => 0x13f,
            Self::Cool => 0x147,
            Self::Slab => 0x14f,
            Self::Fan => 0x157,
            Self::HumidityMin => 0x152,
            Self::HumidityMax => 0x151,
        }
    }
    fn block(self) -> u8 {
        match self {
            Self::Mode => 3,
            Self::Heat => 5,
            Self::Cool => 6,
            Self::Slab => 7,
            Self::Fan => 9,
            Self::HumidityMin => 0xa4,
            Self::HumidityMax => 0xa5,
        }
    }
    fn width(self) -> usize {
        match self {
            Self::Mode => 3,
            Self::Slab => 6,
            _ => 5,
        }
    }
    fn indexed(self) -> bool {
        matches!(self, Self::Heat | Self::Cool | Self::Slab | Self::Fan)
    }
    fn setup(self) -> Option<u8> {
        match self {
            Self::Slab => Some(13),
            Self::HumidityMin | Self::HumidityMax => Some(0xa3),
            _ => None,
        }
    }
}

pub(super) fn slots(setback: u8) -> Option<(usize, u8)> {
    Some(match setback {
        0 | 2 | 4 => (0, 0b00101),
        1 | 3 | 5 => (1, 0b01010),
        6 => (4, 0b10000),
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Idle,
    Read,
    Reading,
    Sample,
    Write,
    Writing,
    Verify,
    Verifying,
    Readback,
    Done(u8),
}

#[derive(Clone, Copy)]
pub(super) struct Operation {
    pub(super) address: u16,
    pub(super) subnet: u8,
    node: u8,
    pub(super) setting: Setting,
    qualified: bool,
    selector: u8,
    write: Option<u8>,
    mask: u8,
    index: usize,
    bytes: [u8; 6],
    since: u32,
    step: Step,
}

impl Operation {
    pub(super) const IDLE: Self = Self {
        address: 0,
        subnet: 0,
        node: 0,
        setting: Setting::Mode,
        qualified: false,
        selector: 0,
        write: None,
        mask: 0,
        index: 0,
        bytes: [0; 6],
        since: 0,
        step: Step::Idle,
    };
    pub(super) fn active(&self) -> bool {
        self.step != Step::Idle
    }
    pub(super) fn begin(
        call: &Call<'_>,
        setting: Setting,
        subnet: u8,
        node: u8,
        setback: u8,
        setbacks: bool,
        now: u32,
    ) -> Option<Self> {
        let (&addr, tail) = call.body.split_first_chunk::<2>()?;
        let (selector, write) = match (setting, call.service, tail) {
            (Setting::Mode, 1, [] | [_] | [_, 0]) => (0, None),
            (Setting::Mode, 0, [v] | [v, 0]) => (0, Some(*v)),
            (Setting::HumidityMin | Setting::HumidityMax, 1, [] | [_]) => (0, None),
            (Setting::HumidityMin | Setting::HumidityMax, 0, [v]) => (0, Some(*v)),
            (_, 1, [s] | [s, _]) if setting.indexed() => (*s, None),
            (_, 0, [s, v]) if setting.indexed() => (*s, Some(*v)),
            _ => return None,
        };
        let selector = if selector >= 7 {
            setback.min(7)
        } else {
            selector
        };
        let selection = if !setting.indexed() {
            Some((0, 1))
        } else {
            slots(selector).map(|(i, mask)| (i, if setbacks { mask } else { 31 }))
        };
        let (index, mask) = selection.unwrap_or_default();
        let valid = selection.is_some()
            && write != Some(255)
            && !(matches!(
                setting,
                Setting::Fan | Setting::HumidityMin | Setting::HumidityMax
            ) && write.is_some_and(|v| v > 100))
            && (setting != Setting::Mode || !matches!(write,Some(v) if !matches!(v,0..=4|6)));
        Some(Self {
            address: u16::from_le_bytes(addr),
            subnet,
            node,
            setting,
            qualified: setting.setup().is_none(),
            selector,
            write,
            mask,
            index,
            bytes: [0; 6],
            since: now,
            step: if valid { Step::Read } else { Step::Done(255) },
        })
    }

    pub(super) fn fail(&mut self) {
        self.step = Step::Done(255);
    }
    pub(super) fn terminal(&self) -> bool {
        matches!(self.step, Step::Done(_))
    }
    pub(super) fn expire(&mut self, now: u32) {
        // Bound bus occupancy even when the line never becomes idle.
        if self.active() && !self.terminal() && now.wrapping_sub(self.since) >= 10 {
            self.fail();
        }
    }
    pub(super) fn take_frame(&mut self, port: u8) -> Option<Request> {
        let block = if self.qualified {
            self.setting.block()
        } else {
            self.setting.setup()?
        };
        match self.step {
            Step::Read | Step::Verify => {
                self.step = if self.step == Step::Read {
                    Step::Reading
                } else {
                    Step::Verifying
                };
                let mut b = [0x6d, 0x70, 0, 7, block, 0, self.subnet, 0, 0, 0, 0];
                b[7..].copy_from_slice(&(1u32 << (self.node - 1)).to_le_bytes());
                Request::new(port, &b)
            }
            Step::Write => {
                self.step = Step::Writing;
                let n = self.setting.width();
                let mut b = [0; 14];
                b[..8].copy_from_slice(&[
                    0x6d,
                    0xf0,
                    0,
                    4 + n as u8,
                    self.subnet,
                    self.node,
                    block,
                    0,
                ]);
                b[8..8 + n].copy_from_slice(&self.bytes[..n]);
                Request::new(port, &b[..8 + n])
            }
            _ => None,
        }
    }
    pub(super) fn sent(&mut self, complete: bool) {
        if !self.active() || self.terminal() {
            return;
        }
        self.step = match (self.step, complete) {
            (Step::Reading, true) => Step::Sample,
            (Step::Writing, true) => Step::Verify,
            (Step::Verifying, true) => Step::Readback,
            _ => Step::Done(255),
        };
    }
    pub(super) fn sample(
        &mut self,
        address: u16,
        subnet: u8,
        data: &[u8],
        live: &super::Telemetry,
    ) {
        let [block, 0, support, values @ ..] = data else {
            return;
        };
        if !self.active() || self.terminal() || self.address != address || self.subnet != subnet {
            return;
        }
        if Some(*block) == self.setting.setup() {
            if !live.configured(self.setting) {
                self.fail();
            } else if !self.qualified && self.step == Step::Sample {
                self.qualified = true;
                self.step = Step::Read;
            }
            return;
        }
        if !self.qualified
            || *block != self.setting.block()
            || !matches!(self.step, Step::Sample | Step::Readback | Step::Write)
        {
            return;
        }
        let Some(actual) = block_byte(*support, values, self.index) else {
            self.fail();
            return;
        };
        let Some(write) = self.write else {
            self.step = Step::Done(actual);
            return;
        };
        if self.step == Step::Readback {
            self.step = Step::Done(actual);
            return;
        }
        let width = self.setting.width();
        // Update carries positional bytes but no support mask. Refuse an
        // incomplete or extended layout rather than fabricating its siblings.
        if values.len() != width || *support >> width != 0 || actual == 255 {
            self.fail();
            return;
        }
        self.bytes[..width].copy_from_slice(values);
        let mut needs_write = false;
        for (i, v) in self.bytes[..width].iter_mut().enumerate() {
            if self.mask & *support & (1 << i) != 0 {
                needs_write |= *v != write;
                *v = write;
            }
        }
        self.step = if needs_write {
            Step::Write
        } else {
            Step::Done(actual)
        };
    }
    pub(super) fn reply(&self) -> Option<Reply> {
        let Step::Done(v) = self.step else {
            return None;
        };
        let [lo, hi] = self.address.to_le_bytes();
        let call = Call {
            service: u8::from(self.write.is_none()),
            method: self.setting.method(),
            body: &[],
        };
        Some(if self.setting.indexed() {
            call.ok(&[lo, hi, self.selector, v])
        } else {
            call.ok(&[lo, hi, v])
        })
    }
}

impl Default for Operation {
    fn default() -> Self {
        Self::IDLE
    }
}
pub(super) fn accept(slot: &mut Operation, op: Operation, call: &Call<'_>) -> Answer {
    if slot.active() {
        return Answer::Reply(call.error(1));
    }
    if let Some(reply) = op.reply() {
        return Answer::Reply(reply);
    }
    *slot = op;
    Answer::Silent
}
