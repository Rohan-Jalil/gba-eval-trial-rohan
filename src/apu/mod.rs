//! Sound controller, IO 0x060-0x0AF (gbatek: GBA Sound Controller).
//!
//! Phase 0 stub: registers are stored and silence is produced at the rate SOUNDBIAS selects,
//! so the host already receives the right number of samples per frame.

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

const SOUNDCNT_X_OFFSET: u32 = 0x084; // SOUNDCNT_X, sound on/off and channel status
const SOUNDCNT_X_MASTER_ENABLE: u16 = 0x0080; // bit 7, PSG/FIFO master enable

pub struct Apu {
    registers: PlainRegisters<0x28>,
    psg: psg::Psg,
    fifos: [fifo::SoundFifo; 2],
    /// Cycles accumulated towards the next output sample.
    cycles_until_sample: u32,
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
            fifos: [fifo::SoundFifo::new(), fifo::SoundFifo::new()],
            cycles_until_sample: 0,
            samples: Vec::with_capacity(MAX_BUFFERED_PAIRS * 2),
        }
    }

    /// Output sample rate: 32768 Hz shifted left by the SOUNDBIAS resolution bits 14-15.
    pub fn sample_rate(&self) -> u32 {
        let resolution = (self.registers.read(SOUNDBIAS) >> 14) & 3;
        BASE_SAMPLE_RATE_HZ << resolution
    }

    /// Advance by `cycles`, appending one stereo pair per output sample period.
    pub fn step(&mut self, cycles: u32) {
        let cycles_per_sample = CPU_CLOCK_HZ / self.sample_rate();
        self.cycles_until_sample += cycles;
        while self.cycles_until_sample >= cycles_per_sample {
            self.cycles_until_sample -= cycles_per_sample;
            let (left, right) = self.mix_sample();
            if self.samples.len() < MAX_BUFFERED_PAIRS * 2 {
                self.samples.push(left);
                self.samples.push(right);
            }
        }
    }

    /// Mix PSG and FIFO channels into one stereo pair. Stub: silence.
    fn mix_sample(&mut self) -> (i16, i16) {
        let _ = (&self.psg, &self.fifos);
        (0, 0)
    }

    /// Timer `timer_index` overflowed. Returns a bit mask of FIFOs (bit 0 = A, bit 1 = B) that
    /// now need a DMA refill; gba.rs forwards those to the DMA controller.
    pub fn on_timer_overflow(&mut self, _timer_index: usize) -> u8 {
        0
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        self.registers.read(offset)
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        // SOUNDCNT_X: only bit 7 (master enable) is writable; bits 0-3 are channel status flags
        // the APU sets itself (gbatek: GBA Sound Control Registers).
        let mask = if offset == SOUNDCNT_X_OFFSET { mask & SOUNDCNT_X_MASTER_ENABLE } else { mask };
        self.registers.write(offset, value, mask)
    }
}
