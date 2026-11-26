#![no_std]
#![no_main]

use board::panel::Panel;
use core::{arch::global_asm, cell::RefCell, panic::PanicInfo, ptr};
use embassy_futures::{
    select::{Either, select},
    yield_now,
};
use embassy_time::Timer;
use static_cell::StaticCell;
use tekmar_482_firmware::{Command, Decoder, IDENTITY, ISP_ACK};

global_asm!(include_str!("startup.S"), options(raw));
mod runtime;

mod board {
    use super::ptr;
    use embassy_futures::yield_now;
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

    pub async fn drain() {
        while read(LSR) & TEMT == 0 {
            yield_now().await;
        }
    }

    pub async fn train() {
        // Do not let the next measurement change the baud of a pending reply.
        drain().await;
        loop {
            write(UART0 + 0x08, 3); // clear stale RX data, leave TX alone
            write(ACR, 0x305); // clear flags, start mode 0, retry on measurement timeout
            // Mode 0 measures the first two falling edges of 'A' and receives the byte.
            if receive().await == Ok(b'A') && receive().await == Ok(b'\n') {
                return;
            }
        }
    }

    pub async fn receive() -> Result<u8, ()> {
        loop {
            let status = read(LSR);
            let byte = (status & 1 != 0).then(|| read(UART0) as u8);
            if status & 0x1e != 0 {
                return Err(());
            }
            if let Some(byte) = byte {
                return Ok(byte);
            }
            yield_now().await;
        }
    }

    pub async fn send(bytes: &[u8]) {
        for chunk in bytes.chunks(16) {
            while read(LSR) & THRE == 0 {
                yield_now().await;
            }
            for &byte in chunk {
                write(UART0, u32::from(byte));
            }
        }
    }

    fn hex(dst: &mut [u8], mut value: u32) {
        for b in dst.iter_mut().rev() {
            *b = b"0123456789abcdef"[(value & 15) as usize];
            value >>= 4;
        }
    }

    // Largest diagnostic line: prefix, fourteen words and 67 hex bytes.
    pub struct Line(heapless::Vec<u8, 272>);

    impl Line {
        pub fn new(prefix: &[u8]) -> Self {
            Self(heapless::Vec::from_slice(prefix).unwrap())
        }
        pub fn words(&mut self, values: impl IntoIterator<Item = u32>) {
            for value in values {
                let mut word = *b" 00000000";
                hex(&mut word[1..], value);
                self.0.extend_from_slice(&word).unwrap();
            }
        }
        fn body(&mut self, bytes: &[u8]) {
            self.0.push(b' ').unwrap();
            for &byte in bytes {
                let mut digits = [0; 2];
                hex(&mut digits, u32::from(byte));
                self.0.extend_from_slice(&digits).unwrap();
            }
        }
        pub async fn send(&mut self) {
            self.0.extend_from_slice(b"\r\n").unwrap();
            send(&self.0).await;
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

async fn next_command() -> Command {
    loop {
        board::train().await;
        let mut decoder = Decoder::default();
        while let Ok(byte) = board::receive().await {
            if let Some(cmd) = decoder.push(byte) {
                return cmd;
            }
            if byte == b'\n' {
                break;
            }
        }
    }
}

#[embassy_executor::task]
async fn indicators(panel: &'static RefCell<Panel>) {
    loop {
        Timer::after_millis(20).await;
        panel.borrow_mut().poll();
    }
}

async fn commands(panel: &RefCell<Panel>, probing: bool) -> Command {
    loop {
        match next_command().await {
            Command::Identify => board::send(IDENTITY).await,
            Command::Build => {
                board::send(concat!("BUILD ", env!("TEKMAR_BUILD_REVISION"), "\r\n").as_bytes())
                    .await
            }
            Command::Panel => {
                let mut line = board::Line::new(b"PANEL");
                line.words(panel.borrow().report());
                line.send().await;
            }
            Command::LedTest => {
                panel.borrow_mut().start_test();
                board::send(b"OK LED\r\n").await;
            }
            Command::BadSend => board::send(b"ERR TX\r\n").await,
            Command::RxEnd if !probing => board::send(b"ERR RX-INACTIVE\r\n").await,
            other => return other,
        }
    }
}

#[embassy_executor::task]
async fn bridge(panel: &'static RefCell<Panel>) {
    let mut probe: Option<board::probe::Capture> = None;
    loop {
        let command = if probe.is_some() {
            commands(panel, true).await
        } else {
            // Cancel background capture (and release both timers) at a mode change.
            let mut idle = board::probe::Receivers::start();
            match select(commands(panel, false), async {
                loop {
                    idle.idle(&mut panel.borrow_mut());
                    yield_now().await;
                }
            })
            .await
            {
                Either::First(command) => command,
                Either::Second(never) => never,
            }
        };
        let previous = probe.take();
        if command == Command::RxEnd {
            if let Some(capture) = previous {
                let mut line = board::Line::new(b"RX");
                line.words(capture.finish());
                line.send().await;
            }
            continue;
        }
        drop(previous);
        match command {
            Command::RxBegin => {
                probe = Some(board::probe::Capture::start());
                board::send(b"OK RX\r\n").await;
            }
            Command::Stream(mode) => board::probe::stream(&mode, panel).await,
            Command::EnterIsp => {
                board::send(ISP_ACK).await;
                board::drain().await;
                panel.borrow_mut().stop();
                board::enter_isp();
            }
            _ => unreachable!(),
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn firmware_main() -> ! {
    static PANEL: StaticCell<RefCell<Panel>> = StaticCell::new();
    board::init();
    let panel = PANEL.init_with(|| RefCell::new(Panel::init()));
    runtime::run(|spawner| {
        spawner.spawn(indicators(panel).unwrap());
        spawner.spawn(bridge(panel).unwrap());
    })
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
