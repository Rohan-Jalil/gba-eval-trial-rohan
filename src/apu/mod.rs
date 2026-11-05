//! Sound controller, IO 0x060-0x0AF (gbatek: GBA Sound Controller).
//!
//! This file is the mixer: SOUNDCNT_L/H/X, SOUNDBIAS, the FIFO registers, the output buffer and
//! sample rate. PSG channel registers (0x060-0x07F, wave RAM 0x090-0x09F) are routed to psg.rs;
//! the mixer steps the PSG and reads `channel_outputs()` (Phase 4 contract).

mod fifo;
mod psg;

use crate::io::PlainRegisters;

const IO_START: u32 = 0x060; // SOUND1CNT_L, first sound register
const SOUNDBIAS: u32 = 0x088; // sound PWM control: bias level and sampling rate
const SOUNDBIAS_DEFAULT: u16 = 0x0200;

/// CPU clock in Hz (16 * 1024 * 1024).
const CPU_CLOCK_HZ: u32 = 16 * 1024 * 1024;
/// Base output rate for SOUNDBIAS resolution 0; each resolution step doubles it.
const BASE_SAMPLE_RATE_HZ: u32 = 32768;
/// Stereo pairs kept when the host never drains; older output is dropped beyond this.
const MAX_BUFFERED_PAIRS: usize = 1 << 16;

const SOUNDCNT_L: u32 = 0x080; // PSG master volume and left/right enables
const SOUNDCNT_H: u32 = 0x082; // PSG ratio, DMA sound volume, enables, timer select, FIFO reset
const SOUNDCNT_X_OFFSET: u32 = 0x084; // SOUNDCNT_X, sound on/off and channel status
const SOUNDCNT_X_MASTER_ENABLE: u16 = 0x0080; // bit 7, PSG/FIFO master enable
const FIFO_A: u32 = 0x0A0; // FIFO_A_L/H, sound A sample bytes (write-only)
const FIFO_B: u32 = 0x0A4; // FIFO_B_L/H, sound B sample bytes (write-only)
/// SOUNDCNT_H bits 11 and 15: reset FIFO A / B (write-only, never stored).
const FIFO_RESET_BITS: [u16; 2] = [1 << 11, 1 << 15];
/// Bias level bits 1-9 of SOUNDBIAS.
const BIAS_LEVEL_MASK: u16 = 0x03FE;
/// Highest value of the unsigned 10-bit DAC.
const DAC_MAX: i32 = 0x3FF;
/// Scale from the 10-bit DAC range (centred on the bias) to i16.
const DAC_TO_I16_SHIFT: u32 = 5;

pub struct Apu {
    registers: PlainRegisters<0x28>,
    psg: psg::Psg,
    /// Cycles the PSG has not been advanced by yet. The PSG only needs to be current when it is
    /// mixed or its registers are accessed, so it is stepped in one go at those points.
    psg_pending_cycles: u32,
    fifos: [fifo::SoundFifo; 2],
    /// Cycles accumulated towards the next output sample.
    cycles_since_sample: u32,
    /// Interleaved stereo i16 output, drained by the host through lib.rs.
    pub samples: Vec<i16>,
}

impl Apu {
    pub fn new() -> Self {
        let mut registers = PlainRegisters::new(IO_START);
        registers.write(SOUNDBIAS, SOUNDBIAS_DEFAULT, 0xFFFF);
        Apu {
            registers,
            psg: psg::Psg::new(),
            psg_pending_cycles: 0,
            fifos: [fifo::SoundFifo::new(), fifo::SoundFifo::new()],
            cycles_since_sample: 0,
            samples: Vec::with_capacity(MAX_BUFFERED_PAIRS * 2),
        }
    }

    /// Output sample rate: 32768 Hz shifted left by the SOUNDBIAS resolution bits 14-15.
    pub fn sample_rate(&self) -> u32 {
        let resolution = (self.registers.read(SOUNDBIAS) >> 14) & 3;
        BASE_SAMPLE_RATE_HZ << resolution
    }

    /// Cycles of `step` until the next output sample is mixed.
    pub fn cycles_until_sample(&self) -> u32 {
        (CPU_CLOCK_HZ / self.sample_rate()).saturating_sub(self.cycles_since_sample)
    }

    /// Advance by `cycles`, appending one stereo pair per output sample period.
    pub fn step(&mut self, cycles: u32) {
        let cycles_per_sample = CPU_CLOCK_HZ / self.sample_rate();
        self.psg_pending_cycles += cycles;
        self.cycles_since_sample += cycles;
        while self.cycles_since_sample >= cycles_per_sample {
            self.cycles_since_sample -= cycles_per_sample;
            self.catch_up_psg();
            let (left, right) = self.mix_sample();
            if self.samples.len() < MAX_BUFFERED_PAIRS * 2 {
                self.samples.push(left);
                self.samples.push(right);
            }
        }
    }

    fn master_enabled(&self) -> bool {
        self.registers.read(SOUNDCNT_X_OFFSET) & SOUNDCNT_X_MASTER_ENABLE != 0
    }

    /// Mix PSG and FIFO channels into one stereo pair
    /// (gbatek: GBA Sound Control Registers, Max Output Levels).
    fn mix_sample(&mut self) -> (i16, i16) {
        let bias_register = self.registers.read(SOUNDBIAS);
        let bias = (bias_register & BIAS_LEVEL_MASK) as i32;
        let (left, right) = if self.master_enabled() { self.mix_channels() } else { (0, 0) };
        let resolution = (bias_register >> 14) as u32;
        (dac_output(left, bias, resolution), dac_output(right, bias, resolution))
    }

    /// Signed sum of all six channels per side, in DAC units (each FIFO spans +/-0x200,
    /// each PSG channel about +/-0x80 at full volume).
    fn mix_channels(&self) -> (i32, i32) {
        let volume_control = self.registers.read(SOUNDCNT_L);
        let mixing_control = self.registers.read(SOUNDCNT_H);
        let right_volume = (volume_control & 7) as i32 + 1;
        let left_volume = ((volume_control >> 4) & 7) as i32 + 1;
        // 0=25%, 1=50%, 2=100%, 3=prohibited (treated as 100%).
        let psg_ratio_shift = 2 - ((mixing_control & 3) as u32).min(2);

        let mut left = 0;
        let mut right = 0;
        for (channel, &level) in self.psg.channel_outputs().iter().enumerate() {
            let level = level as i32;
            if volume_control & (0x0100 << channel) != 0 {
                right += (level * right_volume) >> psg_ratio_shift;
            }
            if volume_control & (0x1000 << channel) != 0 {
                left += (level * left_volume) >> psg_ratio_shift;
            }
        }

        for (fifo_index, fifo) in self.fifos.iter().enumerate() {
            let control_bits = mixing_control >> (8 + 4 * fifo_index);
            let full_volume = mixing_control & (4 << fifo_index) != 0;
            let level = fifo.current_sample as i32 * if full_volume { 4 } else { 2 };
            if control_bits & 1 != 0 {
                right += level;
            }
            if control_bits & 2 != 0 {
                left += level;
            }
        }
        (left, right)
    }

    /// Timer `timer_index` overflowed. Returns a bit mask of FIFOs (bit 0 = A, bit 1 = B) that
    /// now need a DMA refill; gba.rs forwards those to the DMA controller.
    pub fn on_timer_overflow(&mut self, timer_index: usize) -> u8 {
        if !self.master_enabled() {
            return 0;
        }
        let mixing_control = self.registers.read(SOUNDCNT_H);
        let mut refill_requests = 0;
        for (fifo_index, fifo) in self.fifos.iter_mut().enumerate() {
            let selected_timer = ((mixing_control >> (10 + 4 * fifo_index)) & 1) as usize;
            if selected_timer == timer_index && fifo.pop_sample() {
                refill_requests |= 1 << fifo_index;
            }
        }
        refill_requests
    }

    /// Step the PSG by the cycles deferred since it was last brought up to date. Stepping in one
    /// call or many gives the same state (its timers only count down), so this changes no output.
    fn catch_up_psg(&mut self) {
        self.psg.step(std::mem::take(&mut self.psg_pending_cycles));
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        self.catch_up_psg();
        if is_psg_offset(offset) {
            return self.psg.io_read16(offset);
        }
        if offset == SOUNDCNT_X_OFFSET {
            let master = self.registers.read(offset) & SOUNDCNT_X_MASTER_ENABLE;
            return master | self.psg.active_channels() as u16;
        }
        self.registers.read(offset)
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        self.catch_up_psg();
        if is_psg_offset(offset) {
            return self.psg.io_write16(offset, value, mask);
        }
        match offset {
            FIFO_A | 0x0A2 => return self.fifos[0].write16(value, mask),
            FIFO_B | 0x0A6 => return self.fifos[1].write16(value, mask),
            SOUNDCNT_H => return self.write_mixing_control(value, mask),
            SOUNDCNT_X_OFFSET => return self.write_master_enable(value, mask),
            _ => {}
        }
        self.registers.write(offset, value, mask)
    }

    /// SOUNDCNT_H write: the FIFO reset bits act immediately and are not stored.
    fn write_mixing_control(&mut self, value: u16, mask: u16) {
        for (fifo, reset_bit) in self.fifos.iter_mut().zip(FIFO_RESET_BITS) {
            if value & mask & reset_bit != 0 {
                fifo.reset();
            }
        }
        let stored_mask = mask & !(FIFO_RESET_BITS[0] | FIFO_RESET_BITS[1]);
        self.registers.write(SOUNDCNT_H, value, stored_mask)
    }

    /// SOUNDCNT_X write. Clearing the master enable resets the PSG registers 0x060-0x081
    /// (gbatek: GBA Sound Control Registers, SOUNDCNT_X).
    fn write_master_enable(&mut self, value: u16, mask: u16) {
        let was_enabled = self.master_enabled();
        // SOUNDCNT_X: only bit 7 (master enable) is writable; bits 0-3 are channel status flags
        // the APU sets itself (gbatek: GBA Sound Control Registers).
        self.registers.write(SOUNDCNT_X_OFFSET, value, mask & SOUNDCNT_X_MASTER_ENABLE);
        if was_enabled && !self.master_enabled() {
            self.psg.power_off();
            self.registers.write(SOUNDCNT_L, 0, 0xFFFF);
        }
    }
}

/// Add the bias, clip to the unsigned 10-bit DAC, drop the bits the resolution cannot output,
/// and centre on the bias as an i16 (gbatek: SOUNDBIAS, Max Output Levels).
fn dac_output(signed_level: i32, bias: i32, resolution: u32) -> i16 {
    let dac_value = (signed_level + bias).clamp(0, DAC_MAX);
    let kept_bits = dac_value & !((2 << resolution) - 1);
    ((kept_bits - bias) << DAC_TO_I16_SHIFT).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// PSG channel registers and wave RAM belong to psg.rs.
fn is_psg_offset(offset: u32) -> bool {
    (psg::REGISTERS_START..=psg::REGISTERS_END).contains(&offset)
        || (psg::WAVE_RAM_START..=psg::WAVE_RAM_END).contains(&offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_a_plays_at_timer_rate_and_requests_refill() {
        let mut apu = Apu::new();
        apu.io_write16(SOUNDCNT_X_OFFSET, 0x0080, 0xFFFF);
        apu.io_write16(SOUNDCNT_H, 0x0B04 | 0x8000, 0xFFFF); // A full volume, L+R, timer 0, resets
        assert_eq!(apu.io_read16(SOUNDCNT_H), 0x0304);
        apu.io_write16(FIFO_A, 0x7F40, 0xFFFF);
        assert_eq!(apu.on_timer_overflow(1), 0); // wrong timer
        assert_eq!(apu.on_timer_overflow(0), 3); // A has 1 byte left, B is empty: refill both
        apu.step(512);
        // 0x40 * 4 = 0x100 above the 0x200 bias, 9-bit resolution.
        assert_eq!(apu.samples, vec![0x100 << DAC_TO_I16_SHIFT; 2]);
    }

    #[test]
    fn sample_count_follows_soundbias_rate() {
        let mut apu = Apu::new();
        apu.io_write16(SOUNDBIAS, 0xC200, 0xFFFF);
        assert_eq!(apu.sample_rate(), 262144);
        apu.step(CPU_CLOCK_HZ / 64);
        assert_eq!(apu.samples.len(), 2 * 262144 / 64);
    }

    #[test]
    fn dac_clips_to_ten_bits() {
        assert_eq!(dac_output(0x600, 0x200, 0), (0x1FE << DAC_TO_I16_SHIFT) as i16);
        assert_eq!(dac_output(-0x600, 0x200, 0), (-0x200 << DAC_TO_I16_SHIFT) as i16);
    }
}
