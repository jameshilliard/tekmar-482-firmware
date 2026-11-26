//! Thread-mode Embassy runtime for the single-core ARM7. No VIC source is enabled.
use core::arch::asm;
use core::cell::{Cell, RefCell};
use core::task::Waker;
use critical_section::{CriticalSection, Mutex};
use embassy_executor::{Spawner, raw::Executor};
use embassy_time_driver::Driver;
use embassy_time_queue_utils::Queue;
use static_cell::StaticCell;

struct Arm7;
critical_section::set_impl!(Arm7);

unsafe impl critical_section::Impl for Arm7 {
    unsafe fn acquire() -> u32 {
        let saved: u32;
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

struct Clock {
    ticks: Mutex<Cell<u64>>,
    next: Mutex<Cell<u64>>,
    queue: Mutex<RefCell<Queue>>,
}

impl Clock {
    fn sample(&self, cs: CriticalSection<'_>) -> u64 {
        let ticks = self.ticks.borrow(cs);
        let previous = ticks.get();
        let elapsed = crate::board::panel::now().wrapping_sub(previous as u32);
        let next = previous + u64::from(elapsed);
        ticks.set(next);
        next
    }

    fn service(&self) {
        critical_section::with(|cs| {
            let now = self.sample(cs);
            let alarm = self.next.borrow(cs);
            if now >= alarm.get() {
                alarm.set(self.queue.borrow_ref_mut(cs).next_expiration(now));
            }
        });
    }
}

impl Driver for Clock {
    fn now(&self) -> u64 {
        critical_section::with(|cs| self.sample(cs))
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            if self.queue.borrow_ref_mut(cs).schedule_wake(at, waker) {
                let alarm = self.next.borrow(cs);
                alarm.set(alarm.get().min(at));
            }
        });
    }
}

embassy_time_driver::time_driver_impl!(static CLOCK: Clock = Clock {
    ticks: Mutex::new(Cell::new(0)),
    next: Mutex::new(Cell::new(u64::MAX)),
    queue: Mutex::new(RefCell::new(Queue::new())),
});

pub fn run(start: impl FnOnce(Spawner)) -> ! {
    static EXECUTOR: StaticCell<Executor> = StaticCell::new();
    let executor = EXECUTOR.init_with(|| Executor::new(core::ptr::null_mut()));
    start(executor.spawner());
    loop {
        CLOCK.service();
        // Only this thread polls the executor; the spin pender never calls poll.
        unsafe { executor.poll() };
    }
}
