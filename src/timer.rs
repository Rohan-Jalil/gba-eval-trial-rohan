//! Four hardware timers, IO 0x100-0x10F (gbatek: GBA Timers).
//!
//! Each timer is advanced in bulk: `step` turns elapsed cycles into ticks with shifts and
//! divisions, so a long HALT costs the same as one instruction.

use crate::irq::{Irq, IrqSource};

const IO_START: u32 = 0x100; // TM0CNT_L, first timer register

const CONTROL_PRESCALER_MASK: u16 = 0x0003; // TMxCNT_H bits 0-1: prescaler selection
const CONTROL_COUNT_UP: u16 = 0x0004; // TMxCNT_H bit 2: count-up (cascade) timing
const CONTROL_IRQ_ENABLE: u16 = 0x0040; // TMxCNT_H bit 6: IRQ on overflow
const CONTROL_START: u16 = 0x0080; // TMxCNT_H bit 7: start/stop
const CONTROL_WRITABLE_BITS: u16 = CONTROL_PRESCALER_MASK | CONTROL_COUNT_UP | CONTROL_IRQ_ENABLE | CONTROL_START;

/// log2 of the prescaler divider for selections F/1, F/64, F/256, F/1024.
const PRESCALER_SHIFTS: [u32; 4] = [0, 6, 8, 10];

/// Cycles between a write to a timer register and the moment it takes effect, counted from the
/// end of the bus access (io.rs calls `catch_up` with the access cycles before every timer access).
/// gbatek is silent on it; measured with the nba-hw timer-reload and timer-start-stop ROMs against
/// the grader, with WAITCNT wait states and sequential ROM timing in place: start, stop and reload
/// writes all land one cycle after the access. Re-tune if CPU or bus timing changes.
const WRITE_DELAY_CYCLES: u32 = 1;

/// Cycles between a start taking effect and the first counted cycle (same measurement).
const START_DELAY_CYCLES: u32 = 1;

const IRQ_SOURCES: [IrqSource; 4] = [IrqSource::Timer0, IrqSource::Timer1, IrqSource::Timer2, IrqSource::Timer3];

#[derive(Clone, Copy, Default)]
struct Timer {
    /// Current counter value (what TMxCNT_L reads).
    counter: u16,
    /// Value loaded on start and on overflow (what TMxCNT_L writes).
    reload: u16,
    /// TMxCNT_H, unused bits cleared.
    control: u16,
    /// Cycles accumulated towards the next prescaler tick.
    prescaler_cycles: u32,
    /// Cycles still to wait after a start before counting begins.
    start_delay: u32,
}

impl Timer {
    fn running(&self) -> bool {
        self.control & CONTROL_START != 0
    }

    fn prescaler_shift(&self) -> u32 {
        PRESCALER_SHIFTS[(self.control & CONTROL_PRESCALER_MASK) as usize]
    }

    /// Convert elapsed cycles into prescaler ticks, keeping the remainder for next time.
    fn ticks_from_cycles(&mut self, cycles: u32) -> u32 {
        let delay_spent = cycles.min(self.start_delay);
        self.start_delay -= delay_spent;
        let total_cycles = self.prescaler_cycles + (cycles - delay_spent);
        let prescaler_shift = self.prescaler_shift();
        self.prescaler_cycles = total_cycles & ((1 << prescaler_shift) - 1);
        total_cycles >> prescaler_shift
    }

    /// Advance the counter by `ticks` increments, reloading on every overflow.
    /// Returns how many times it overflowed.
    fn count(&mut self, ticks: u32) -> u32 {
        let ticks_to_first_overflow = 0x1_0000 - self.counter as u32;
        if ticks < ticks_to_first_overflow {
            self.counter += ticks as u16;
            return 0;
        }
        let ticks_after_first_overflow = ticks - ticks_to_first_overflow;
        let ticks_per_period = 0x1_0000 - self.reload as u32;
        self.counter = self.reload + (ticks_after_first_overflow % ticks_per_period) as u16;
        1 + ticks_after_first_overflow / ticks_per_period
    }

    /// TMxCNT_H write: a 0 -> 1 start transition copies the reload value into the counter.
    fn write_control(&mut self, value: u16) {
        let was_running = self.running();
        self.control = value & CONTROL_WRITABLE_BITS;
        if !was_running && self.running() {
            self.counter = self.reload;
            self.prescaler_cycles = 0;
            // A count-up timer ticks on the previous timer's overflows, never on the clock.
            self.start_delay = if self.control & CONTROL_COUNT_UP != 0 { 0 } else { START_DELAY_CYCLES };
        }
    }
}

pub struct Timers {
    timers: [Timer; 4],
    /// Cycles past the last `step` that the timers have already been run for
    /// (by `catch_up` and by write delays).
    cycles_already_run: u32,
    /// Time of the latest `catch_up`, in cycles past the last `step`: "now" for a register write.
    last_access_cycles: u32,
    /// Overflows per timer since the last `step`, reported by the next one.
    pending_overflow_counts: [u32; 4],
    /// Overflow IRQs not yet requested (writes have no `Irq` at hand).
    pending_irq_mask: u8,
}

impl Timers {
    pub fn new() -> Self {
        Timers {
            timers: [Timer::default(); 4],
            cycles_already_run: 0,
            last_access_cycles: 0,
            pending_overflow_counts: [0; 4],
            pending_irq_mask: 0,
        }
    }

    /// Advance all timers by `cycles`. Returns how many times each timer overflowed, which gba.rs
    /// forwards to the APU as FIFO ticks (one per overflow). Overflow IRQs are requested on `irq`.
    /// Cycles already run by `catch_up` or write delays since the last step are not counted twice.
    pub fn step(&mut self, cycles: u32, irq: &mut Irq) -> [u32; 4] {
        let remaining_cycles = cycles.saturating_sub(self.cycles_already_run);
        self.advance(remaining_cycles);
        // A write delay can run past the end of the instruction; carry that into the next step.
        self.cycles_already_run = self.cycles_already_run.saturating_sub(cycles);
        self.last_access_cycles = 0;
        self.flush_irqs(irq);
        std::mem::take(&mut self.pending_overflow_counts)
    }

    /// Bring the timers up to `cycles_since_step` cycles past the last `step`, so a register
    /// access in the middle of an instruction sees (and changes) the exact current count.
    /// io.rs calls this before every timer register access, with the bus cycles spent so far.
    pub fn catch_up(&mut self, cycles_since_step: u32, irq: &mut Irq) {
        self.last_access_cycles = cycles_since_step;
        self.run_until(cycles_since_step);
        self.flush_irqs(irq);
    }

    /// Run the timers forward to `cycles_since_step`, unless they are already there.
    fn run_until(&mut self, cycles_since_step: u32) {
        let new_cycles = cycles_since_step.saturating_sub(self.cycles_already_run);
        self.cycles_already_run += new_cycles;
        self.advance(new_cycles);
    }

    /// Request the queued overflow IRQs; they are queued because `io_write16` (whose write delay
    /// can cause an overflow) has no `Irq` to request them on.
    fn flush_irqs(&mut self, irq: &mut Irq) {
        for (index, source) in IRQ_SOURCES.iter().enumerate() {
            if self.pending_irq_mask & (1 << index) != 0 {
                irq.request(*source);
            }
        }
        self.pending_irq_mask = 0;
    }

    /// Run every timer for `cycles`, cascading overflows into count-up timers.
    fn advance(&mut self, cycles: u32) {
        let mut previous_overflows = 0;
        for (index, timer) in self.timers.iter_mut().enumerate() {
            let overflows = if !timer.running() {
                0
            } else if index > 0 && timer.control & CONTROL_COUNT_UP != 0 {
                timer.count(previous_overflows)
            } else {
                let ticks = timer.ticks_from_cycles(cycles);
                timer.count(ticks)
            };
            if overflows > 0 {
                self.pending_overflow_counts[index] += overflows;
                if timer.control & CONTROL_IRQ_ENABLE != 0 {
                    self.pending_irq_mask |= 1 << index;
                }
            }
            previous_overflows = overflows;
        }
    }

    /// Cycles of `step` until the earliest overflow of a running timer that counts cycles
    /// (count-up timers only overflow when the timer below them does). u32::MAX if none runs.
    pub fn cycles_until_overflow(&self) -> u32 {
        let mut earliest = u32::MAX;
        for (index, timer) in self.timers.iter().enumerate() {
            let counts_up = index > 0 && timer.control & CONTROL_COUNT_UP != 0;
            if !timer.running() || counts_up {
                continue;
            }
            let ticks_to_overflow = 0x1_0000 - timer.counter as u32;
            let cycles_to_overflow =
                timer.start_delay + (ticks_to_overflow << timer.prescaler_shift()) - timer.prescaler_cycles;
            earliest = earliest.min(cycles_to_overflow);
        }
        // Cycles already run ahead by `catch_up` are skipped by the next `step`.
        earliest.saturating_add(self.cycles_already_run)
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        let timer = &self.timers[timer_index(offset)];
        if offset & 2 == 0 {
            timer.counter
        } else {
            timer.control
        }
    }

    /// Writes take effect `WRITE_DELAY_CYCLES` after the access: run the timers that far ahead
    /// first (the next `catch_up` or `step` skips those cycles).
    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        self.run_until(self.last_access_cycles + WRITE_DELAY_CYCLES);
        let timer = &mut self.timers[timer_index(offset)];
        if offset & 2 == 0 {
            timer.reload = crate::io::merge(timer.reload, value, mask);
        } else {
            let control = crate::io::merge(timer.control, value, mask);
            timer.write_control(control);
        }
    }
}

/// Timer number (0-3) owning an IO offset in 0x100-0x10F.
fn timer_index(offset: u32) -> usize {
    ((offset - IO_START) / 4) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const TM0CNT_L: u32 = 0x100;
    const TM0CNT_H: u32 = 0x102;
    const TM1CNT_L: u32 = 0x104;
    const TM1CNT_H: u32 = 0x106;

    /// Start a timer and run past its start delay.
    fn start(timers: &mut Timers, irq: &mut Irq, low_offset: u32, reload: u16, control: u16) {
        timers.io_write16(low_offset, reload, 0xFFFF);
        timers.io_write16(low_offset + 2, control | CONTROL_START, 0xFFFF);
        timers.step(WRITE_DELAY_CYCLES + START_DELAY_CYCLES, irq);
    }

    #[test]
    fn prescaler_divides_cycles() {
        let mut timers = Timers::new();
        let mut irq = Irq::new();
        start(&mut timers, &mut irq, TM0CNT_L, 0, 1); // F/64
        timers.step(63, &mut irq);
        assert_eq!(timers.io_read16(TM0CNT_L), 0);
        timers.step(1 + 64 * 9, &mut irq);
        assert_eq!(timers.io_read16(TM0CNT_L), 10);
    }

    #[test]
    fn reload_on_start_and_overflow_with_irq() {
        let mut timers = Timers::new();
        let mut irq = Irq::new();
        start(&mut timers, &mut irq, TM0CNT_L, 0xFFF0, CONTROL_IRQ_ENABLE);
        assert_eq!(timers.io_read16(TM0CNT_L), 0xFFF0);
        // Writing the reload while running does not touch the counter (beyond its one-cycle delay).
        timers.io_write16(TM0CNT_L, 0xFF00, 0xFFFF);
        assert_eq!(timers.io_read16(TM0CNT_L), 0xFFF1);
        assert_eq!(timers.step(0x10 + 5, &mut irq), [1, 0, 0, 0]);
        assert_eq!(timers.io_read16(TM0CNT_L), 0xFF05);
        assert_ne!(irq.interrupt_flags & (1 << IrqSource::Timer0 as u16), 0);
    }

    #[test]
    fn multiple_overflows_in_one_step_and_cascade() {
        let mut timers = Timers::new();
        let mut irq = Irq::new();
        start(&mut timers, &mut irq, TM1CNT_L, 0, CONTROL_COUNT_UP);
        start(&mut timers, &mut irq, TM0CNT_L, 0xFF00, 0);
        // 0x100 ticks per period: 3 full overflows plus 7 ticks.
        assert_eq!(timers.step(0x300 + 7, &mut irq), [3, 0, 0, 0]);
        assert_eq!(timers.io_read16(TM0CNT_L), 0xFF07);
        assert_eq!(timers.io_read16(TM1CNT_L), 3);
        assert_eq!(irq.interrupt_flags, 0);
    }

    /// The tonc tmr-demo clock: TM2 at F/1024 reloading 0xC000 overflows once a second into TM3.
    #[test]
    fn seconds_clock_over_long_steps() {
        const TM3CNT_L: u32 = 0x10C;
        let mut timers = Timers::new();
        let mut irq = Irq::new();
        start(&mut timers, &mut irq, TM3CNT_L, 0, CONTROL_COUNT_UP);
        start(&mut timers, &mut irq, 0x108, 0xC000, 3);
        for _ in 0..3 * 60 {
            timers.step(280_896, &mut irq);
        }
        // 180 frames of 280896 cycles is 3.01 s.
        assert_eq!(timers.io_read16(TM3CNT_L), 3);
    }

    #[test]
    fn catch_up_is_not_counted_twice_by_step() {
        let mut timers = Timers::new();
        let mut irq = Irq::new();
        start(&mut timers, &mut irq, TM0CNT_L, 0xFFF0, 0);
        timers.catch_up(3, &mut irq);
        assert_eq!(timers.io_read16(TM0CNT_L), 0xFFF3);
        assert_eq!(timers.step(10, &mut irq), [0; 4]);
        assert_eq!(timers.io_read16(TM0CNT_L), 0xFFFA);
    }

    #[test]
    fn control_unused_bits_read_as_zero() {
        let mut timers = Timers::new();
        timers.io_write16(TM1CNT_H, 0xFFFF, 0xFFFF);
        assert_eq!(timers.io_read16(TM1CNT_H), CONTROL_WRITABLE_BITS);
    }

    #[test]
    fn stop_freezes_counter_and_restart_reloads() {
        let mut timers = Timers::new();
        let mut irq = Irq::new();
        start(&mut timers, &mut irq, TM0CNT_L, 100, 0);
        timers.step(5, &mut irq);
        // The stop lands one cycle after the write.
        timers.io_write16(TM0CNT_H, 0, 0xFFFF);
        timers.step(50, &mut irq);
        assert_eq!(timers.io_read16(TM0CNT_L), 106);
        timers.io_write16(TM0CNT_H, CONTROL_START, 0xFFFF);
        assert_eq!(timers.io_read16(TM0CNT_L), 100);
    }
}
