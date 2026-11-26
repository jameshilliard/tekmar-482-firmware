#![no_std]
#![no_main]

use core::{arch::global_asm, panic::PanicInfo, ptr};
use tekmar_482_firmware::{Command, Decoder, IDENTITY, ISP_ACK};

global_asm!(include_str!("startup.S"), options(raw));

mod board {
    use super::ptr;
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

    pub fn train(tick: &mut impl FnMut()) {
        // Do not let the next measurement change the baud of a pending reply.
        while read(LSR) & TEMT == 0 {}
        loop {
            write(UART0 + 0x08, 3); // clear stale RX data, leave TX alone
            write(ACR, 0x305); // clear flags, start mode 0, retry on measurement timeout
            // Mode 0 measures the first two falling edges of 'A' and receives the byte.
            if receive(tick) == Ok(b'A') && receive(tick) == Ok(b'\n') {
                return;
            }
        }
    }

    pub fn receive(tick: &mut impl FnMut()) -> Result<u8, ()> {
        loop {
            let status = read(LSR);
            let byte = (status & 1 != 0).then(|| read(UART0) as u8);
            if status & 0x1e != 0 {
                return Err(());
            }
            if let Some(byte) = byte {
                return Ok(byte);
            }
            tick();
        }
    }

    pub fn send(bytes: &[u8]) {
        for &byte in bytes {
            while read(LSR) & THRE == 0 {}
            write(UART0, u32::from(byte));
        }
    }

    fn send_word(value: u32) {
        let mut word = *b" 00000000";
        for (i, b) in word[1..].iter_mut().rev().enumerate() {
            *b = b"0123456789abcdef"[((value >> (4 * i)) & 15) as usize];
        }
        send(&word);
    }

    pub fn enter_isp() -> ! {
        while read(LSR) & TEMT == 0 {} // Wait for the last stop bit, not just FIFO space.
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

fn next_command(mut tick: impl FnMut()) -> Command {
    loop {
        board::train(&mut tick);
        let mut decoder = Decoder::default();
        while let Ok(byte) = board::receive(&mut tick) {
            if let Some(cmd) = decoder.push(byte) {
                return cmd;
            }
            if byte == b'\n' {
                break;
            }
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn firmware_main() -> ! {
    board::init();
    let mut panel = board::panel::Panel::init();
    let mut probe: Option<board::probe::Capture> = None;
    loop {
        // Exactly one owner of the capture timers: background, edge probe, or stream.
        let mut idle = probe.is_none().then(board::probe::Receivers::start);
        let command = loop {
            let command = next_command(|| {
                if let Some(bus) = &mut idle {
                    bus.idle(&mut panel);
                }
                panel.poll();
            });
            match command {
                Command::Identify => board::send(IDENTITY),
                Command::Panel => {
                    panel.poll();
                    panel.report();
                }
                Command::LedTest => {
                    panel.start_test();
                    board::send(b"OK LED\r\n");
                }
                Command::BadSend => board::send(b"ERR TX\r\n"),
                Command::RxEnd if probe.is_none() => board::send(b"ERR RX-INACTIVE\r\n"),
                other => break other,
            }
        };
        drop(idle);
        let previous = probe.take();
        if command == Command::RxEnd {
            if let Some(capture) = previous {
                capture.report();
            }
            continue;
        }
        drop(previous);
        match command {
            Command::RxBegin => {
                probe = Some(board::probe::Capture::start());
                board::send(b"OK RX\r\n");
            }
            Command::RxStream | Command::RxDiagnostics => {
                board::probe::stream(command == Command::RxDiagnostics, &mut panel, None);
            }
            Command::Send(request) => board::probe::stream(true, &mut panel, Some(&request)),
            Command::EnterIsp => {
                panel.stop();
                board::send(ISP_ACK);
                board::enter_isp();
            }
            _ => unreachable!(),
        }
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
