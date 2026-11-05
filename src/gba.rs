//! The whole machine: CPU plus bus (which owns every other component), and the frame loop.
//!
//! Events between components (PPU -> DMA, timers -> APU, APU -> DMA) are plain method calls here,
//! so no component holds a reference to another.

use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::dma;
use crate::irq::IrqSource;
use crate::ppu::CYCLES_PER_LINE;
use crate::scheduler::Scheduler;

/// 228 lines x 1232 cycles (gbatek: LCD Dimensions and Timings).
pub const CYCLES_PER_FRAME: u32 = 280_896;

/// Scanline at whose start `run_frame` returns. 0 = each frame runs lines 0..227 (drawing, then
/// all of VBlank); 160 would end at VBlank start. Measured against Mesen2 via the grader (see
/// NOTES.md, 2026-11-05): a key set before frame N first shows in frame N+1 on tonc key-demo,
/// which polls keys at VBlank, so the reference's frame includes the VBlank after the picture.
pub const FRAME_END_LINE: u32 = 0;

pub struct Gba {
    pub cpu: Cpu,
    pub bus: Bus,
    pub scheduler: Scheduler,
    /// Cycles left before the current frame ends; overshoot carries into the next frame.
    cycles_until_frame_end: i64,
}

impl Gba {
    /// Power-on state with `rom` inserted. The CPU starts at the BIOS reset vector.
    pub fn new(rom: &[u8]) -> Self {
        let first_frame_cycles = match FRAME_END_LINE * CYCLES_PER_LINE {
            0 => CYCLES_PER_FRAME,
            cycles => cycles,
        };
        Gba {
            cpu: Cpu::new(),
            bus: Bus::new(rom),
            scheduler: Scheduler::new(),
            cycles_until_frame_end: first_frame_cycles as i64,
        }
    }

    /// Latch the host key state (ABI bit order, active high).
    pub fn set_keys(&mut self, pressed_keys: u32) {
        self.bus.keypad.set_keys(pressed_keys);
    }

    /// Run until the frame boundary (see `FRAME_END_LINE`).
    pub fn run_frame(&mut self) {
        // Keypad IRQ (gbatek: GBA Keypad Input, KEYCNT). Keys only change between frames.
        // ponytail: checked once per frame; a mid-frame KEYCNT write waits until the next frame.
        if self.bus.keypad.irq_condition_met() {
            self.bus.irq.request(IrqSource::Keypad);
        }
        while self.cycles_until_frame_end > 0 {
            let mut cycles = self.cpu.step(&mut self.bus);
            cycles += dma::run_pending_transfers(&mut self.bus);
            self.advance_components(cycles);
            self.cycles_until_frame_end -= cycles as i64;
        }
        self.cycles_until_frame_end += CYCLES_PER_FRAME as i64;
    }

    /// Advance PPU, timers and APU by `cycles` and route their events.
    fn advance_components(&mut self, cycles: u32) {
        let bus = &mut self.bus;
        self.scheduler.advance(cycles);

        let ppu_events = bus.ppu.step(cycles, &mut bus.irq);
        if ppu_events.hblank_started {
            bus.dma.notify_hblank();
        }
        if ppu_events.vblank_started {
            bus.dma.notify_vblank();
        }

        let overflowed_timers = bus.timers.step(cycles, &mut bus.irq);
        for timer_index in 0..4 {
            if overflowed_timers & (1 << timer_index) == 0 {
                continue;
            }
            let fifo_requests = bus.apu.on_timer_overflow(timer_index);
            for fifo_index in 0..2 {
                if fifo_requests & (1 << fifo_index) != 0 {
                    bus.dma.notify_fifo_request(fifo_index);
                }
            }
        }

        bus.apu.step(cycles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_keep_line_timing_aligned() {
        let mut gba = Gba::new(&[]);
        gba.run_frame();
        assert_eq!(gba.bus.ppu.vcount as u32, FRAME_END_LINE % 228);
        let after_first = gba.scheduler.total_cycles;
        gba.run_frame();
        assert_eq!(gba.scheduler.total_cycles - after_first, CYCLES_PER_FRAME as u64);
        assert_eq!(gba.bus.ppu.vcount as u32, FRAME_END_LINE % 228);
    }

    #[test]
    fn vblank_irq_is_requested_when_enabled() {
        let mut gba = Gba::new(&[]);
        gba.bus.write16(0x0400_0004, 1 << 3); // DISPSTAT: VBlank IRQ enable
        gba.run_frame();
        gba.run_frame();
        assert_ne!(gba.bus.irq.interrupt_flags & 1, 0);
    }
}
