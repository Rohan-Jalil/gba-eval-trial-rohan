//! PSG sound channels 1-4: tone+sweep, tone, wave, noise (gbatek: GBA Sound Channel 1-4).
//!
//! Phase 4 contract (stub): the mixer in apu/mod.rs routes the PSG registers here, steps the
//! channels with the CPU cycles that elapsed, and reads each channel's current output level.
//! Volume (SOUNDCNT_L), panning and the PSG/FIFO ratio (SOUNDCNT_H) are applied by the mixer.

use crate::io::PlainRegisters;

/// PSG register block: SOUND1CNT_L (0x060) through SOUND4CNT_H (0x07E), then wave RAM 0x090-0x09F.
pub const REGISTERS_START: u32 = 0x060;
pub const REGISTERS_END: u32 = 0x07F;
pub const WAVE_RAM_START: u32 = 0x090;
pub const WAVE_RAM_END: u32 = 0x09F;

pub struct Psg {
    registers: PlainRegisters<0x20>,
    wave_ram: PlainRegisters<0x8>,
}

impl Psg {
    pub fn new() -> Self {
        Psg { registers: PlainRegisters::new(REGISTERS_START), wave_ram: PlainRegisters::new(WAVE_RAM_START) }
    }

    /// Advance every channel by `cycles` CPU cycles (length, envelope, sweep, frequency timers).
    pub fn step(&mut self, _cycles: u32) {}

    /// Current output of channels 1-4 as signed levels in -15..=15 (envelope or wave volume
    /// applied, 0 when the channel is off). The mixer scales and pans them.
    pub fn channel_outputs(&self) -> [i16; 4] {
        [0; 4]
    }

    /// SOUNDCNT_X bits 0-3: which channels are currently playing.
    pub fn active_channels(&self) -> u8 {
        0
    }

    #[allow(dead_code)] // called by the mixer (Phase 4 fifo-mixer worker)
    /// SOUNDCNT_X master enable cleared: reset the PSG registers (gbatek: SOUNDCNT_X bit 7).
    pub fn power_off(&mut self) {
        self.registers = PlainRegisters::new(REGISTERS_START);
    }

    /// Offsets in 0x060-0x07F or 0x090-0x09F.
    pub fn io_read16(&mut self, offset: u32) -> u16 {
        if offset >= WAVE_RAM_START { self.wave_ram.read(offset) } else { self.registers.read(offset) }
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        if offset >= WAVE_RAM_START {
            self.wave_ram.write(offset, value, mask)
        } else {
            self.registers.write(offset, value, mask)
        }
    }
}
