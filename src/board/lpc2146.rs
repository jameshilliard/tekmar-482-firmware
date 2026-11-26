//! LPC2146 register fields used by this board (UM10139).
pub const PCLK_HZ: u32 = 48_000_000;
pub const PRESCALE_MICROSECONDS: u32 = PCLK_HZ / 1_000_000 - 1;

pub mod uart0 {
    pub const BASE: usize = 0xe000_c000;
    pub const DATA: usize = BASE; // RBR or THR; DLL when DLAB is set
    pub const IER: usize = BASE + 0x04;
    pub const DLM: usize = IER;
    pub const FCR: usize = BASE + 0x08;
    pub const LCR: usize = BASE + 0x0c;
    pub const LSR: usize = BASE + 0x14;
    pub const ACR: usize = BASE + 0x20;
    pub const FDR: usize = BASE + 0x28;
    pub const TER: usize = BASE + 0x30;
    pub const FORMAT_8N1: u32 = 3;
    pub const DLAB: u32 = 1 << 7;
    pub const RX_READY: u32 = 1;
    pub const OVERRUN: u32 = 1 << 1;
    pub const PARITY: u32 = 1 << 2;
    pub const FRAMING: u32 = 1 << 3;
    pub const BREAK: u32 = 1 << 4;
    pub const RX_ERRORS: u32 = OVERRUN | PARITY | FRAMING | BREAK;
    pub const THRE: u32 = 1 << 5;
    pub const TEMT: u32 = 1 << 6;
    pub const AUTOBAUD_START: u32 = 1;
    pub const AUTOBAUD_MODE_1: u32 = 1 << 1;
    pub const CLEAR_AUTOBAUD_FLAGS: u32 = 3 << 8;
    pub const FIFO_ENABLE_AND_RESET: u32 = 0b111;
    pub const FIFO_BYTES: usize = 16;
    pub const TX_ENABLE: u32 = 1 << 7;
    pub const INTEGER_DIVIDER: u32 = 1 << 4; // MULVAL=1, DIVADDVAL=0
    pub const CLOCKS_PER_BIT: u32 = 16;
}

pub mod gpio {
    pub const FIO0: usize = 0x3fff_c000;
    pub const FIO1: usize = FIO0 + 0x20;
    pub const MASK: usize = 0x10;
    pub const PIN: usize = 0x14;
    pub const SET: usize = 0x18;
    pub const CLR: usize = 0x1c;
    pub const RXD0: u32 = 1 << 1;
    pub const MUX_BITS: usize = 2;
    pub const MUX_MASK: u32 = 3;
    pub const PINS_PER_MUX_REGISTER: usize = u32::BITS as usize / MUX_BITS;
}

pub mod timer {
    pub const CONTROL: usize = 0x04;
    pub const COUNTER: usize = 0x08;
    pub const PRESCALE: usize = 0x0c;
    pub const MATCH_CONTROL: usize = 0x14;
    pub const ENABLE: u32 = 1;
    pub const RESET: u32 = 1 << 1;
    pub const FIRST_CAPTURE_IRQ: usize = 4;
    pub const CAPTURE_RISING: u32 = 1;
    pub const CAPTURE_FALLING: u32 = 1 << 1;
    pub const CAPTURE_IRQ: u32 = 1 << 2;
    pub const CAPTURE_FIELD_BITS: usize = 3;
    pub const CAPTURE_FIELD_MASK: u32 = (1 << CAPTURE_FIELD_BITS) - 1;
    pub const CLEAR_ALL_IRQS: u32 = 0xff;
}
