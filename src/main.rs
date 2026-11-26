#![no_std]
#![no_main]

use board::panel::Panel;
use core::{arch::global_asm, cell::RefCell, panic::PanicInfo, ptr};
use embassy_time::Timer;
use static_cell::StaticCell;

global_asm!(include_str!("startup.S"), options(raw));
mod runtime;

mod board {
    use super::ptr;
    pub(super) mod baud;
    mod lpc2146;
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
    use lpc2146::uart0;
    const PLL_ENABLE: u32 = 1;
    const PLL_CONNECT: u32 = 1 << 1;
    const PLL_LOCKED: u32 = 1 << 10;
    const CRYSTAL_HZ: u32 = 12_000_000;
    const PLL_MULTIPLIER: u32 = lpc2146::PCLK_HZ / CRYSTAL_HZ;
    const PLL_PSEL_DIV2: u32 = 1 << 5;
    const PLL_FEED_SEQUENCE: [u32; 2] = [0xaa, 0x55];
    const MAM_FETCH_CLOCKS: u32 = 3;
    const APB_SAME_AS_CPU: u32 = 1;
    const UART0_POWER: u32 = 1 << 3;
    const UART0_PIN_MASK: u32 = 0xf;
    const UART0_PIN_FUNCTIONS: u32 = 0x5; // TXD0 / RXD0
    const IAP_REINVOKE_ISP: u32 = 57;

    // Private helpers: callers supply aligned LPC2146 peripheral registers only.
    fn read(addr: usize) -> u32 {
        unsafe { ptr::read_volatile(addr as *const u32) }
    }
    fn write(addr: usize, value: u32) {
        unsafe { ptr::write_volatile(addr as *mut u32, value) }
    }

    fn pll_control(value: u32) {
        write(PLL0CON, value);
        for byte in PLL_FEED_SEQUENCE {
            write(PLL0FEED, byte);
        }
    }

    pub fn init() {
        // A ROM Go into SRAM can leave its final reply in UART0's shift register.
        #[cfg(feature = "ram-trial")]
        while read(uart0::LSR) & uart0::TEMT == 0 {}
        pll_control(0);
        // UART autobaud at 187500 needs more than the crystal alone (UM10139 10.3.13).
        write(MAMCR, 0);
        write(MAMTIM, MAM_FETCH_CLOCKS);
        write(PLL0CFG, PLL_PSEL_DIV2 | (PLL_MULTIPLIER - 1));
        pll_control(PLL_ENABLE);
        while read(PLL0STAT) & PLL_LOCKED == 0 {}
        pll_control(PLL_ENABLE | PLL_CONNECT);
        write(APBDIV, APB_SAME_AS_CPU);
        write(PCONP, read(PCONP) | UART0_POWER);
        write(
            PINSEL0,
            (read(PINSEL0) & !UART0_PIN_MASK) | UART0_PIN_FUNCTIONS,
        );
        write(uart0::FDR, uart0::INTEGER_DIVIDER);
        write(uart0::LCR, uart0::FORMAT_8N1);
        write(uart0::IER, 0);
        write(uart0::FCR, uart0::FIFO_ENABLE_AND_RESET);
        write(uart0::TER, uart0::TX_ENABLE);
    }

    pub fn try_receive() -> Result<Option<u8>, ()> {
        let status = read(uart0::LSR);
        let byte = (status & uart0::RX_READY != 0).then(|| read(uart0::DATA) as u8);
        if status & uart0::RX_ERRORS != 0 {
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
        let command = [IAP_REINVOKE_ISP, 0, 0, 0, 0];
        let mut result = [0; 5];
        // Startup masks IRQ/FIQ; the command tables and stack are in user RAM.
        unsafe { rom_iap(command.as_ptr(), result.as_mut_ptr()) }
    }
}

#[embassy_executor::task]
async fn indicators(panel: &'static RefCell<Panel>) {
    loop {
        Timer::after_millis(board::panel::SAMPLE_MS).await;
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
