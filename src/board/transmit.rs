use super::{PINSEL0, lpc2146::gpio, read, write};
use tekmar_482_firmware::tn4::PORT_COUNT;
use tekmar_482_firmware::tn4::tx::{Action, Mode, Outcome, Request, Sender};

const FIO: usize = gpio::FIO0;
const MASK: usize = FIO + gpio::MASK;
const PIN: usize = FIO + gpio::PIN;
const SET: usize = FIO + gpio::SET;
const CLR: usize = FIO + gpio::CLR;
const TX_PINS: [usize; PORT_COUNT] = [28, 13, 22, 8, 29];

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
        if read(FIO) & tx != 0
            || (read(PINSEL0 + size_of::<u32>() * (pin / gpio::PINS_PER_MUX_REGISTER))
                >> (gpio::MUX_BITS * (pin % gpio::PINS_PER_MUX_REGISTER)))
                & gpio::MUX_MASK
                != 0
        {
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
    fn release(&mut self) {
        if self.enabled {
            write(CLR, self.tx);
            write(FIO, read(FIO) & !self.tx);
            self.enabled = false;
        }
    }
}

impl Drop for LineDriver {
    fn drop(&mut self) {
        self.release();
        write(
            MASK,
            (read(MASK) & !(self.tx | self.rx)) | (self.mask & (self.tx | self.rx)),
        );
    }
}

impl Attempt {
    pub(super) fn started(&self) -> bool {
        self.sender.started()
    }

    pub(super) fn respond(&mut self, dominant: bool) {
        debug_assert!(!self.started());
        if dominant {
            self.pin.drive(true);
        } else {
            self.pin.release();
        }
    }

    pub(super) fn new(request: &Request, rx_pin: u32, now: u32, mode: Mode) -> Option<Self> {
        Some(Self {
            pin: LineDriver::claim(request.port, rx_pin)?,
            sender: Sender::new(request, now, mode),
        })
    }

    pub(super) fn yield_arbitration(&mut self, now: u32) {
        self.pin.release();
        let reset = self.sender.retry_arbitration(now);
        debug_assert!(reset);
    }

    pub(super) fn poll(&mut self, now: u32, may_start: bool) -> Option<(Outcome, Option<u32>)> {
        if !may_start && self.sender.hold_if_waiting(now) {
            return None;
        }
        match self.sender.poll(now, read(PIN) & self.pin.rx != 0) {
            Action::Drive(high) => {
                self.pin.drive(high);
                None
            }
            Action::Finish(outcome) => Some((outcome, None)),
            Action::Fault { outcome, next_bit } => Some((outcome, Some(next_bit))),
            Action::None => None,
        }
    }
}
