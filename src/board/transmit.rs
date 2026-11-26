use super::{PINSEL0, read, write};
use tekmar_482_firmware::tn4::tx::{Action, Outcome, Request, Sender};

const FIO: usize = 0x3fff_c000;
const MASK: usize = FIO + 0x10;
const PIN: usize = FIO + 0x14;
const SET: usize = FIO + 0x18;
const CLR: usize = FIO + 0x1c;
const TX_PINS: [usize; 5] = [28, 13, 22, 8, 29];

pub(super) struct Attempt {
    sender: Sender,
    pin: LineDriver,
}

pub(super) struct LineDriver {
    tx: u32,
    rx: u32,
    mask: u32,
    enabled: bool,
}

impl LineDriver {
    pub(super) fn claim(port: u8, rx_pin: u32) -> Option<Self> {
        let pin = TX_PINS[usize::from(port)];
        let tx = 1 << pin;
        let rx = 1 << rx_pin;
        if read(FIO) & tx != 0 || (read(PINSEL0 + 4 * (pin / 16)) >> (2 * (pin % 16))) & 3 != 0 {
            return None; // not an unclaimed GPIO input
        }
        let mask = read(MASK);
        write(MASK, mask & !(rx | tx));
        Some(Self {
            tx,
            rx,
            mask,
            enabled: false,
        })
    }

    pub(super) fn drive(&mut self, high: bool) {
        write(if high { SET } else { CLR }, self.tx);
        if !self.enabled {
            write(FIO, read(FIO) | self.tx);
            self.enabled = true;
        }
    }
}

impl Drop for LineDriver {
    fn drop(&mut self) {
        if self.enabled {
            write(CLR, self.tx);
            write(FIO, read(FIO) & !self.tx);
        }
        write(
            MASK,
            (read(MASK) & !(self.tx | self.rx)) | (self.mask & (self.tx | self.rx)),
        );
    }
}

impl Attempt {
    pub(super) fn started(&self) -> bool {
        self.pin.enabled
    }

    pub(super) fn new(request: &Request, rx_pin: u32, now: u32) -> Option<Self> {
        Some(Self {
            pin: LineDriver::claim(request.port, rx_pin)?,
            sender: Sender::new(request, now),
        })
    }

    pub(super) fn poll(&mut self, now: u32) -> Option<Outcome> {
        match self.sender.poll(now, read(PIN) & self.pin.rx != 0) {
            Action::Drive(high) => {
                self.pin.drive(high);
                None
            }
            Action::Finish(outcome) => Some(outcome),
            Action::None => None,
        }
    }
}
