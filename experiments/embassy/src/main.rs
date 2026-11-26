#![no_std]
#![no_main]

use core::arch::{asm, global_asm};
use core::cell::{Cell, RefCell};
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};
use core::task::Waker;
use critical_section::Mutex;
use embassy_executor::raw::Executor;
use embassy_time::{Duration, Instant, Timer};
use embassy_time_driver::Driver;
use embassy_time_queue_utils::Queue;
use static_cell::StaticCell;

global_asm!(include_str!("../../../src/startup.S"), options(raw));

struct Arm7;
critical_section::set_impl!(Arm7);

unsafe impl critical_section::Impl for Arm7 {
    unsafe fn acquire() -> u32 {
        let saved: u32;
        // Save and mask IRQ and FIQ. The asm memory barrier also orders shared data.
        unsafe {
            asm!("mrs {saved}, cpsr", "orr {tmp}, {saved}, #0xc0", "msr cpsr_c, {tmp}",
                 saved = out(reg) saved, tmp = out(reg) _, options(nostack, preserves_flags));
        }
        saved
    }

    unsafe fn release(saved: u32) {
        unsafe { asm!("msr cpsr_c, {}", in(reg) saved, options(nostack, preserves_flags)) };
    }
}

const T0: usize = 0xe000_4000;
const PCONP: usize = 0xe01f_c0c4;
const VPBDIV: usize = 0xe01f_c100;
const UART: usize = 0xe000_c000;

fn read(addr: usize) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}

fn write(addr: usize, value: u32) {
    unsafe { write_volatile(addr as *mut u32, value) }
}

struct PolledTimer {
    ticks: Mutex<Cell<u64>>,
    queue: Mutex<RefCell<Queue>>,
}

impl Driver for PolledTimer {
    fn now(&self) -> u64 {
        critical_section::with(|cs| {
            let ticks = self.ticks.borrow(cs);
            let previous = ticks.get();
            let elapsed = read(T0 + 8).wrapping_sub(previous as u32);
            let next = previous + u64::from(elapsed);
            ticks.set(next);
            next
        })
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| self.queue.borrow_ref_mut(cs).schedule_wake(at, waker));
    }
}

embassy_time_driver::time_driver_impl!(static CLOCK: PolledTimer = PolledTimer {
    ticks: Mutex::new(Cell::new(0)),
    queue: Mutex::new(RefCell::new(Queue::new())),
});

fn send(byte: u8) {
    while read(UART + 0x14) & (1 << 5) == 0 {}
    write(UART, u32::from(byte));
}

#[embassy_executor::task(pool_size = 2)]
async fn marker(start: Instant, period_ms: u64, count: u32, byte: u8) {
    for i in 1..=count {
        Timer::at(start + Duration::from_millis(period_ms * u64::from(i))).await;
        send(byte);
    }
}

fn return_to_rom() -> ! {
    unsafe extern "C" {
        fn rom_iap(command: *const u32, result: *mut u32) -> !;
    }
    unsafe { rom_iap([57, 0, 0, 0, 0].as_ptr(), [0; 5].as_mut_ptr()) }
}

#[unsafe(no_mangle)]
extern "C" fn firmware_main() -> ! {
    // Entered only via ROM Go, with the 12 MHz crystal and the ISP UART intact.
    let power = read(PCONP);
    let divider = match read(VPBDIV) & 3 {
        0 => 4,
        1 => 1,
        2 => 2,
        _ => return_to_rom(),
    };
    write(PCONP, power | (1 << 1));
    write(T0 + 4, 2);
    write(T0 + 0x70, 0); // timer, not external counter
    write(T0 + 0x14, 0); // no match resets or interrupts
    write(T0 + 0x0c, 12_000 / divider - 1);
    write(T0 + 4, 1);

    static EXECUTOR: StaticCell<Executor> = StaticCell::new();
    let executor = EXECUTOR.init_with(|| Executor::new(core::ptr::null_mut()));
    let start = Instant::now();
    executor
        .spawner()
        .spawn(marker(start, 100, 3, b'F').unwrap());
    executor
        .spawner()
        .spawn(marker(start, 225, 1, b'S').unwrap());
    while start.elapsed() < Duration::from_millis(400) {
        let now = CLOCK.now();
        critical_section::with(|cs| CLOCK.queue.borrow_ref_mut(cs).next_expiration(now));
        // Single executor, polled only here. Its spin pender never calls poll.
        unsafe { executor.poll() };
    }
    send(b'\r');
    send(b'\n');
    while read(UART + 0x14) & (1 << 6) == 0 {}
    write(T0 + 4, 0);
    write(PCONP, power);
    return_to_rom()
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    return_to_rom()
}
