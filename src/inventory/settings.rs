//! A host operation owns one port until its device reply has been queued.
//! All writes start from a fresh block, never from the temperature target.
use super::codes::{mode, setback, setpoint};
const MAX_PERCENT: u16 = 100;
const UNAVAILABLE_BYTE: u16 = super::codes::UNAVAILABLE as u16;
const COMMAND_TIMEOUT_SECONDS: u32 = 10;
const WORD_PAIR_MASK: u8 = 0b11;
const ALL_TARGET_WORD_BYTES: u8 = 0x3f;
const TARGET_WORD_LO_BYTES: u8 = 0x15;
const OPTIONAL_MAX_WORD_BYTES: u8 = 0xc0;
use super::{Answer, BridgeError, block, block_byte, block_request, message, method, service};
use crate::{
    tn4::tx::{Outcome, Request},
    trpc::{Call, Reply},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Setting {
    Mode,
    DeviceTarget,
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
            method::MODE_SETTING => Self::Mode,
            method::SETPOINT_DEVICE => Self::DeviceTarget,
            method::HEAT_SETPOINT => Self::Heat,
            method::COOL_SETPOINT => Self::Cool,
            method::SLAB_SETPOINT => Self::Slab,
            method::FAN_PERCENT => Self::Fan,
            method::HUMIDITY_MIN => Self::HumidityMin,
            method::HUMIDITY_MAX => Self::HumidityMax,
            _ => return None,
        })
    }
    fn method(self) -> u32 {
        match self {
            Self::Mode => method::MODE_SETTING,
            Self::DeviceTarget => method::SETPOINT_DEVICE,
            Self::Heat => method::HEAT_SETPOINT,
            Self::Cool => method::COOL_SETPOINT,
            Self::Slab => method::SLAB_SETPOINT,
            Self::Fan => method::FAN_PERCENT,
            Self::HumidityMin => method::HUMIDITY_MIN,
            Self::HumidityMax => method::HUMIDITY_MAX,
        }
    }
    pub(super) fn block(self, app_mode: Option<u8>) -> Option<u8> {
        Some(match self {
            Self::DeviceTarget => match app_mode? {
                setpoint::ON_OFF | setpoint::PWM => block::SETPOINT_TARGETS,
                setpoint::HOT_TUB => block::HOT_TUB_TARGETS,
                setpoint::DHW => block::DHW_TARGETS,
                setpoint::FLOOR => block::FLOOR_TARGETS,
                setpoint::POOL => block::POOL_TARGETS,
                _ => return None,
            },
            Self::Mode => block::HEATING_SETUP,
            Self::Heat => block::HEAT_SETPOINTS,
            Self::Cool => block::COOL_SETPOINTS,
            Self::Slab => block::SLAB_SETPOINTS,
            Self::Fan => block::FAN_PERCENT,
            Self::HumidityMin => block::HUMIDIFY_TARGETS,
            Self::HumidityMax => block::DEHUMIDIFY_TARGETS,
        })
    }
    fn width(self) -> usize {
        match self {
            Self::Mode => 3, // mode, heating CPH, air group
            Self::DeviceTarget => 8,
            Self::Slab => 6,
            _ => 5,
        }
    }
    fn indexed(self) -> bool {
        matches!(
            self,
            Self::Heat | Self::Cool | Self::Slab | Self::Fan | Self::DeviceTarget
        )
    }
    fn setup(self) -> Option<u8> {
        match self {
            Self::Slab => Some(block::SENSOR_SETUP),
            Self::DeviceTarget => Some(block::SETPOINT_SETUP),
            Self::HumidityMin | Self::HumidityMax => Some(block::HUMIDITY_SETUP),
            _ => None,
        }
    }
}

pub(super) fn slots(setback: u8) -> Option<(usize, u8)> {
    Some(setback::Band::from_selector(setback)?.thermostat_slots())
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
    Done(u16),
}

#[derive(Clone, Copy)]
pub(super) struct Operation {
    pub(super) address: u16,
    pub(super) subnet: u8,
    node: u8,
    pub(super) setting: Setting,
    block: u8,
    application: Option<u8>,
    selector: u8,
    write: Option<u16>,
    mask: u8,
    index: usize,
    bytes: [u8; 8],
    length: usize,
    since: u32,
    step: Step,
}

impl Operation {
    pub(super) const IDLE: Self = Self {
        address: 0,
        subnet: 0,
        node: 0,
        setting: Setting::Mode,
        block: 0,
        application: None,
        selector: 0,
        write: None,
        mask: 0,
        index: 0,
        bytes: [0; 8],
        length: 0,
        since: 0,
        step: Step::Idle,
    };
    pub(super) fn active(&self) -> bool {
        self.step != Step::Idle
    }
    pub(super) fn frame_waiting(&self) -> bool {
        matches!(self.step, Step::Read | Step::Write | Step::Verify)
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
            (Setting::Mode, service::REQUEST, [] | [_] | [_, 0]) => (0, None),
            (Setting::Mode, service::UPDATE, [v] | [v, 0]) => (0, Some(u16::from(*v))),
            (Setting::HumidityMin | Setting::HumidityMax, service::REQUEST, [] | [_]) => (0, None),
            (Setting::HumidityMin | Setting::HumidityMax, service::UPDATE, [v]) => {
                (0, Some(u16::from(*v)))
            }
            (Setting::DeviceTarget, service::REQUEST, [s] | [s, _, _]) => (*s, None),
            (Setting::DeviceTarget, service::UPDATE, [s, a, b]) => {
                (*s, Some(u16::from_le_bytes([*a, *b])))
            }
            (Setting::DeviceTarget, _, _) => return None,
            (_, service::REQUEST, [s] | [s, _]) if setting.indexed() => (*s, None),
            (_, service::UPDATE, [s, v]) if setting.indexed() => (*s, Some(u16::from(*v))),
            _ => return None,
        };
        let selector = if selector >= setback::UNKNOWN {
            setback.min(setback::UNKNOWN)
        } else {
            selector
        };
        let selection = if !setting.indexed() {
            Some((0, 1))
        } else if setting == Setting::DeviceTarget {
            setback::Band::from_selector(selector).map(|band| {
                let i = band.target_word_offset();
                (
                    i,
                    if setbacks {
                        WORD_PAIR_MASK << i
                    } else {
                        ALL_TARGET_WORD_BYTES
                    },
                )
            })
        } else {
            slots(selector).map(|(i, mask)| (i, if setbacks { mask } else { setback::ALL_SLOTS }))
        };
        let (index, mask) = selection.unwrap_or_default();
        let valid = selection.is_some()
            && write
                != Some(if setting == Setting::DeviceTarget {
                    u16::MAX
                } else {
                    UNAVAILABLE_BYTE
                })
            && !(matches!(
                setting,
                Setting::Fan | Setting::HumidityMin | Setting::HumidityMax
            ) && write.is_some_and(|v| v > MAX_PERCENT))
            && (setting != Setting::Mode
                || write
                    .is_none_or(|v| v <= u16::from(mode::VENT) || v == u16::from(mode::EMERGENCY)));
        Some(Self {
            address: u16::from_le_bytes(addr),
            subnet,
            node,
            setting,
            block: setting.setup().or_else(|| setting.block(None))?,
            application: None,
            selector,
            write,
            mask,
            index,
            bytes: [0; 8],
            length: 0,
            since: now,
            step: if valid {
                Step::Read
            } else {
                Step::Done(u16::MAX)
            },
        })
    }

    pub(super) fn fail(&mut self) {
        self.step = Step::Done(u16::MAX);
    }
    pub(super) fn terminal(&self) -> bool {
        matches!(self.step, Step::Done(_))
    }
    pub(super) fn expire(&mut self, now: u32) -> bool {
        // Bound bus occupancy even when the line never becomes idle.
        if self.active()
            && !self.terminal()
            && now.wrapping_sub(self.since) >= COMMAND_TIMEOUT_SECONDS
        {
            self.fail();
            return true;
        }
        false
    }
    pub(super) fn mode_read_node(&self) -> Option<usize> {
        (self.setting == Setting::Mode && self.write.is_none()).then(|| usize::from(self.node - 1))
    }
    pub(super) fn take_frame(&mut self, port: u8) -> Option<Request> {
        let block = self.block;
        match self.step {
            Step::Read | Step::Verify => {
                self.step = if self.step == Step::Read {
                    Step::Reading
                } else {
                    Step::Verifying
                };
                Request::new(
                    port,
                    &block_request(block, self.subnet, usize::from(self.node - 1)),
                )
            }
            Step::Write => {
                self.step = Step::Writing;
                let n = self.length;
                const UPDATE_PREFIX: usize = 2 + size_of::<u16>(); // destination and block ID
                let mut payload = [0; UPDATE_PREFIX + 8];
                payload[..UPDATE_PREFIX].copy_from_slice(&[self.subnet, self.node, block, 0]);
                payload[UPDATE_PREFIX..UPDATE_PREFIX + n].copy_from_slice(&self.bytes[..n]);
                Request::gateway(port, message::BLOCK_UPDATE, &payload[..UPDATE_PREFIX + n])
            }
            _ => None,
        }
    }
    pub(super) fn sent(&mut self, outcome: Outcome) {
        if !self.active() || self.terminal() {
            return;
        }
        self.step = match (self.step, outcome) {
            (Step::Reading, Outcome::ArbitrationLost) => Step::Read,
            (Step::Verifying, Outcome::ArbitrationLost) => Step::Verify,
            (Step::Writing, Outcome::ArbitrationLost) => {
                // The winner may have changed a sibling byte or reassigned the
                // application. Rebuild from fresh evidence, within the same deadline.
                if let Some(setup) = self.setting.setup() {
                    self.block = setup;
                }
                Step::Read
            }
            (Step::Reading, Outcome::Ack | Outcome::NoAck) => Step::Sample,
            (Step::Verifying, Outcome::Ack | Outcome::NoAck) => Step::Readback,
            (Step::Writing, _) => Step::Verify, // possibly delivered; only retry the read
            _ => Step::Done(u16::MAX),
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
            let app = block_byte(*support, values, 0);
            let Some(next) = self.setting.block(app) else {
                self.fail();
                return;
            };
            if !live.configured(self.setting)
                || self.application.is_some() && self.application != app
                || self.block != *block && next != self.block
            {
                self.fail(); // also cancel if the application changed during a transaction
            } else if self.block == *block && self.step == Step::Sample {
                if self.setting == Setting::DeviceTarget {
                    self.application = app;
                }
                self.block = next;
                self.step = Step::Read;
            }
            return;
        }
        if *block != self.block || !matches!(self.step, Step::Sample | Step::Readback | Step::Write)
        {
            return;
        }
        let word = self.setting == Setting::DeviceTarget;
        let value = block_byte(*support, values, self.index).and_then(|a| {
            if word {
                block_byte(*support, values, self.index + 1).map(|b| u16::from_le_bytes([a, b]))
            } else {
                Some(u16::from(a))
            }
        });
        let Some(actual) = value else {
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
        // The 483 layout allows floor controllers to omit the maximum word.
        let width = if self.block == block::FLOOR_TARGETS
            && values.len() == 6
            && support & OPTIONAL_MAX_WORD_BYTES == 0
        {
            6
        } else {
            self.setting.width()
        };
        // Update carries positional bytes but no support mask. Refuse an
        // incomplete or extended layout rather than fabricating its siblings.
        if values.len() != width
            || u16::from(*support) >> width != 0
            || actual == if word { u16::MAX } else { UNAVAILABLE_BYTE }
        {
            self.fail();
            return;
        }
        self.bytes[..width].copy_from_slice(values);
        self.length = width;
        let bytes = write.to_le_bytes();
        let mask = self.mask & support;
        if word && (mask ^ (mask >> 1)) & TARGET_WORD_LO_BYTES != 0 {
            self.fail(); // never write only half of a target
            return;
        }
        let mut needs_write = false;
        for (i, v) in self.bytes[..width].iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                let new = bytes[if word { i % 2 } else { 0 }];
                needs_write |= *v != new;
                *v = new;
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
            service: if self.write.is_some() {
                service::UPDATE
            } else {
                service::REQUEST
            },
            method: self.setting.method(),
            body: &[],
        };
        let [a, b] = v.to_le_bytes();
        Some(if self.setting == Setting::DeviceTarget {
            call.ok(&[lo, hi, self.selector, a, b])
        } else if self.setting.indexed() {
            call.ok(&[lo, hi, self.selector, a])
        } else {
            call.ok(&[lo, hi, a])
        })
    }
}

impl Default for Operation {
    fn default() -> Self {
        Self::IDLE
    }
}
pub(super) fn accept(slot: &mut Operation, op: Operation, call: &Call<'_>) -> Answer {
    if let Some(reply) = op.reply() {
        return Answer::Reply(reply);
    }
    if slot.active() {
        return Answer::Reply(call.error(BridgeError::Busy));
    }
    *slot = op;
    Answer::Silent
}
