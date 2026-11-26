#![no_std]
#![no_main]

use board::panel::Panel;
use core::{arch::global_asm, cell::RefCell, panic::PanicInfo, ptr};
use embassy_time::Timer;
use static_cell::StaticCell;

global_asm!(include_str!("startup.S"), options(raw));
mod runtime;

#[used]
#[unsafe(link_section = ".revision")]
static REVISION: [u8; 48] = {
    let mut field = [0; 48];
    let magic = b"TK48REV\0";
    let rev = env!("TEKMAR_BUILD_REVISION").as_bytes();
    let mut i = 0;
    while i < magic.len() {
        field[i] = magic[i];
        i += 1;
    }
    let mut j = 0;
    while j < rev.len() {
        field[i + j] = rev[j];
        j += 1;
    }
    field
};

mod board {
    use super::ptr;
    pub(super) mod baud;
    mod output;
    pub mod panel;
    pub mod probe;
    mod transmit;

    const MAMCR: usize = 0xe01f_c000;
    const MAMTIM: usize = 0xe01f_c004;
    const PLL0CON: usize = 0xe01f_c080;
    const PLL0CFG: usize = 0xe01f_c084;
    const PLL0STAT: usize = 0xe01f_c088;
    const PLL0FEED: usize = 0xe01f_c08c;
    const PCONP: usize = 0xe01f_c0c4;
    const APBDIV: usize = 0xe01f_c100;
    const PINSEL0: usize = 0xe002_c000;
    const UART0: usize = 0xe000_c000;
    const LSR: usize = UART0 + 0x14;
    const ACR: usize = UART0 + 0x20;
    const THRE: u32 = 1 << 5;
    const TEMT: u32 = 1 << 6;

    // Private helpers: callers supply aligned LPC2146 peripheral registers only.
    fn read(addr: usize) -> u32 {
        unsafe { ptr::read_volatile(addr as *const u32) }
    }
    fn write(addr: usize, value: u32) {
        unsafe { ptr::write_volatile(addr as *mut u32, value) }
    }

    fn pll_control(value: u32) {
        write(PLL0CON, value);
        write(PLL0FEED, 0xaa);
        write(PLL0FEED, 0x55);
    }

    pub fn init() {
        // A ROM Go into SRAM can leave its final reply in UART0's shift register.
        #[cfg(feature = "ram-trial")]
        while read(LSR) & TEMT == 0 {}
        pll_control(0);
        // UART autobaud at 187500 needs more than the crystal alone (UM10139 10.3.13).
        write(MAMCR, 0);
        write(MAMTIM, 3);
        write(PLL0CFG, 0x23); // M=4, P=2: CCLK=48 MHz, FCCO=192 MHz
        pll_control(1);
        while read(PLL0STAT) & (1 << 10) == 0 {}
        pll_control(3);
        write(APBDIV, 1);
        write(PCONP, read(PCONP) | (1 << 3));
        write(PINSEL0, (read(PINSEL0) & !0xf) | 0x5);
        write(UART0 + 0x28, 0x10); // no fractional divider
        write(UART0 + 0x0c, 0x03);
        write(UART0 + 0x04, 0); // polling, no UART interrupts
        write(UART0 + 0x08, 7); // enable and clear FIFOs
        write(UART0 + 0x30, 0x80); // enable transmitter
    }

    pub fn try_receive() -> Result<Option<u8>, ()> {
        let status = read(LSR);
        let byte = (status & 1 != 0).then(|| read(UART0) as u8);
        if status & 0x1e != 0 {
            Err(())
        } else {
            Ok(byte)
        }
    }

    pub fn enter_isp() -> ! {
        pll_control(0); // ROM ISP expects to run from the crystal.
        unsafe extern "C" {
            fn rom_iap(command: *const u32, result: *mut u32) -> !;
        }
        let command = [57, 0, 0, 0, 0];
        let mut result = [0; 5];
        // Startup masks IRQ/FIQ; the command tables and stack are in user RAM.
        unsafe { rom_iap(command.as_ptr(), result.as_mut_ptr()) }
    }
}

#[embassy_executor::task]
async fn indicators(panel: &'static RefCell<Panel>) {
    loop {
        Timer::after_millis(20).await;
        panel.borrow_mut().poll();
    }
}

#[embassy_executor::task]
async fn gateway(panel: &'static RefCell<Panel>) {
    board::probe::run(panel).await;
}

#[unsafe(no_mangle)]
extern "C" fn firmware_main() -> ! {
    static PANEL: StaticCell<RefCell<Panel>> = StaticCell::new();
    board::init();
    let panel = PANEL.init_with(|| RefCell::new(Panel::init()));
    runtime::run(|spawner| {
        spawner.spawn(indicators(panel).unwrap());
        board::baud::set_divisor(board::baud::DEFAULT_DIVISOR);
        spawner.spawn(gateway(panel).unwrap());
    })
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
