//! PSG sound channels 1-4: tone+sweep, tone, wave, noise (gbatek: GBA Sound Channel 1-4).
//!
//! The mixer in apu/mod.rs routes the PSG registers here, steps the channels with the CPU cycles
//! that elapsed, and reads each channel's current output level. Volume (SOUNDCNT_L), panning and
//! the PSG/FIFO ratio (SOUNDCNT_H) are applied by the mixer.

use crate::io::PlainRegisters;

/// PSG register block: SOUND1CNT_L (0x060) through SOUND4CNT_H (0x07E), then wave RAM 0x090-0x09F.
pub const REGISTERS_START: u32 = 0x060;
pub const REGISTERS_END: u32 = 0x07F;
pub const WAVE_RAM_START: u32 = 0x090;
pub const WAVE_RAM_END: u32 = 0x09F;

const SOUND1CNT_L: u32 = 0x060; // channel 1 sweep
const SOUND1CNT_H: u32 = 0x062; // channel 1 duty, length, envelope
const SOUND1CNT_X: u32 = 0x064; // channel 1 frequency, control
const SOUND2CNT_L: u32 = 0x068; // channel 2 duty, length, envelope
const SOUND2CNT_H: u32 = 0x06C; // channel 2 frequency, control
const SOUND3CNT_L: u32 = 0x070; // channel 3 stop, wave RAM select
const SOUND3CNT_H: u32 = 0x072; // channel 3 length, volume
const SOUND3CNT_X: u32 = 0x074; // channel 3 sample rate, control
const SOUND4CNT_L: u32 = 0x078; // channel 4 length, envelope
const SOUND4CNT_H: u32 = 0x07C; // channel 4 frequency, control

/// CPU cycles per frame-sequencer tick (16.78 MHz / 512 Hz).
const FRAME_SEQUENCER_PERIOD: u32 = 32768;
/// Bit 15 of NRx4: restart the sound.
const TRIGGER_BIT: u16 = 0x8000;
/// Bit 14 of NRx4: stop when the length counter expires.
const LENGTH_ENABLE_BIT: u16 = 0x4000;

/// Duty patterns, one bit per step, step 0 first (gbatek: SOUND1CNT_H wave duty).
const DUTY_PATTERNS: [[bool; 8]; 4] = [
    [true, false, false, false, false, false, false, false],
    [true, true, false, false, false, false, false, false],
    [true, true, true, true, false, false, false, false],
    [true, true, true, true, true, true, false, false],
];

/// Countdown timer that fires every `period` cycles; returns how many times it fired.
fn advance_timer(countdown: &mut u32, period: u32, cycles: u32) -> u32 {
    if cycles < *countdown {
        *countdown -= cycles;
        return 0;
    }
    let remaining = cycles - *countdown;
    *countdown = period - remaining % period;
    1 + remaining / period
}

/// Length counter shared by all channels (gbatek: "Sound length" and NRx4 "Length Flag").
#[derive(Default)]
struct LengthCounter {
    remaining: u16,
    enabled: bool,
}

impl LengthCounter {
    /// 256 Hz tick. Returns true when the channel must stop.
    fn tick(&mut self) -> bool {
        if !self.enabled || self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        self.remaining == 0
    }

    /// On restart an expired counter reloads to its full length.
    fn reload_if_expired(&mut self, full_length: u16) {
        if self.remaining == 0 {
            self.remaining = full_length;
        }
    }
}

/// Volume envelope for channels 1, 2, 4 (gbatek: SOUND1CNT_H bits 8-15).
#[derive(Default)]
struct Envelope {
    volume: u8,
    countdown: u8,
}

impl Envelope {
    fn restart(&mut self, control: u16) {
        self.volume = envelope_initial_volume(control);
        self.countdown = envelope_step_time(control);
    }

    /// 64 Hz tick: step the volume once every `step_time` ticks, clamped to 0..=15.
    fn tick(&mut self, control: u16) {
        let step_time = envelope_step_time(control);
        if step_time == 0 {
            return;
        }
        self.countdown = self.countdown.saturating_sub(1);
        if self.countdown > 0 {
            return;
        }
        self.countdown = step_time;
        let increase = control & 0x0800 != 0;
        if increase && self.volume < 15 {
            self.volume += 1;
        } else if !increase && self.volume > 0 {
            self.volume -= 1;
        }
    }
}

fn envelope_step_time(control: u16) -> u8 {
    ((control >> 8) & 7) as u8
}

fn envelope_initial_volume(control: u16) -> u8 {
    (control >> 12) as u8
}

/// A channel with volume 0 and direction "decrease" produces no sound and is switched off.
fn envelope_silences_channel(control: u16) -> bool {
    control & 0xF800 == 0
}

/// Frequency sweep state of channel 1 (gbatek: SOUND1CNT_L).
#[derive(Default)]
struct Sweep {
    shadow_frequency: u16,
    countdown: u8,
    enabled: bool,
}

/// Channel 1 or 2 square wave (gbatek: GBA Sound Channel 1 - Tone & Sweep).
#[derive(Default)]
struct ToneChannel {
    playing: bool,
    frequency: u16,
    duty_step: u8,
    countdown: u32,
    length: LengthCounter,
    envelope: Envelope,
    sweep: Sweep,
}

impl ToneChannel {
    /// CPU cycles per duty step: 131072/(2048-n) Hz with 8 steps per period.
    fn step_period(&self) -> u32 {
        (2048 - self.frequency as u32) * 16
    }

    fn advance(&mut self, cycles: u32) {
        let period = self.step_period();
        let steps = advance_timer(&mut self.countdown, period, cycles);
        self.duty_step = ((self.duty_step as u32 + steps) % 8) as u8;
    }

    fn output(&self, duty_envelope: u16) -> i16 {
        if !self.playing {
            return 0;
        }
        let duty = ((duty_envelope >> 6) & 3) as usize;
        let level = self.envelope.volume as i16;
        if DUTY_PATTERNS[duty][self.duty_step as usize] { level } else { -level }
    }

    fn restart(&mut self, duty_envelope: u16) {
        self.playing = !envelope_silences_channel(duty_envelope);
        self.length.reload_if_expired(64);
        self.envelope.restart(duty_envelope);
        self.countdown = self.step_period();
    }

    /// Next sweep frequency X(t) = X(t-1) +/- X(t-1)/2^n.
    fn sweep_target(&self, sweep_control: u16) -> u16 {
        let shift = sweep_control & 7;
        let delta = self.sweep.shadow_frequency >> shift;
        if sweep_control & 0x0008 != 0 {
            self.sweep.shadow_frequency - delta
        } else {
            self.sweep.shadow_frequency + delta
        }
    }

    /// Restart part of the sweep: latch the frequency and check for an immediate overflow.
    fn restart_sweep(&mut self, sweep_control: u16) {
        let sweep_time = ((sweep_control >> 4) & 7) as u8;
        let shift = sweep_control & 7;
        self.sweep.shadow_frequency = self.frequency;
        self.sweep.countdown = if sweep_time == 0 { 8 } else { sweep_time };
        self.sweep.enabled = sweep_time != 0 || shift != 0;
        if shift != 0 && self.sweep_target(sweep_control) > 2047 {
            self.playing = false;
        }
    }

    /// 128 Hz tick: apply one sweep step every `sweep_time` ticks; overflow past 2047 stops it.
    fn tick_sweep(&mut self, sweep_control: u16) {
        let sweep_time = ((sweep_control >> 4) & 7) as u8;
        self.sweep.countdown = self.sweep.countdown.saturating_sub(1);
        if self.sweep.countdown > 0 {
            return;
        }
        self.sweep.countdown = if sweep_time == 0 { 8 } else { sweep_time };
        if !self.sweep.enabled || sweep_time == 0 {
            return;
        }
        let target = self.sweep_target(sweep_control);
        if target > 2047 {
            self.playing = false;
            return;
        }
        if sweep_control & 7 != 0 {
            self.sweep.shadow_frequency = target;
            self.frequency = target;
            if self.sweep_target(sweep_control) > 2047 {
                self.playing = false;
            }
        }
    }
}

/// Channel 3 wave output (gbatek: GBA Sound Channel 3 - Wave Output).
#[derive(Default)]
struct WaveChannel {
    playing: bool,
    frequency: u16,
    /// Digit position within the played sequence: 0..32, or 0..64 in two-bank mode.
    position: u8,
    countdown: u32,
    length: LengthCounter,
}

impl WaveChannel {
    /// CPU cycles per wave digit: 2097152/(2048-n) Hz.
    fn digit_period(&self) -> u32 {
        (2048 - self.frequency as u32) * 8
    }

    fn advance(&mut self, cycles: u32, digit_count: u32) {
        let period = self.digit_period();
        let steps = advance_timer(&mut self.countdown, period, cycles);
        self.position = ((self.position as u32 + steps) % digit_count) as u8;
    }
}

/// Channel 4 noise (gbatek: GBA Sound Channel 4 - Noise).
#[derive(Default)]
struct NoiseChannel {
    playing: bool,
    shift_register: u16,
    output_high: bool,
    countdown: u32,
    length: LengthCounter,
    envelope: Envelope,
}

/// CPU cycles per noise shift: 524288 Hz / r / 2^(s+1), with r=0 treated as 0.5.
fn noise_period(control: u16) -> u32 {
    let ratio = (control & 7) as u32;
    let shift_clock = ((control >> 4) & 0xF) as u32;
    let base = if ratio == 0 { 16 } else { 32 * ratio };
    base << (shift_clock + 1)
}

impl NoiseChannel {
    fn advance(&mut self, cycles: u32, control: u16) {
        let steps = advance_timer(&mut self.countdown, noise_period(control), cycles);
        let tap = if control & 0x0008 != 0 { 0x60 } else { 0x6000 };
        // ponytail: one loop iteration per shift; at most cycles/32 iterations per call.
        for _ in 0..steps {
            let carry = self.shift_register & 1 != 0;
            self.shift_register >>= 1;
            if carry {
                self.shift_register ^= tap;
            }
            self.output_high = carry;
        }
    }

    fn restart(&mut self, envelope_control: u16, control: u16) {
        self.playing = !envelope_silences_channel(envelope_control);
        self.length.reload_if_expired(64);
        self.envelope.restart(envelope_control);
        self.shift_register = if control & 0x0008 != 0 { 0x40 } else { 0x4000 };
        self.countdown = noise_period(control);
    }
}

pub struct Psg {
    registers: PlainRegisters<0x20>,
    /// Both 16-byte wave RAM banks: bank 0 at 0..16, bank 1 at 16..32.
    wave_banks: [u8; 32],
    tone_channels: [ToneChannel; 2],
    wave: WaveChannel,
    noise: NoiseChannel,
    /// Frame sequencer step 0-7 and cycles until the next 512 Hz tick.
    frame_step: u8,
    frame_countdown: u32,
}

impl Psg {
    pub fn new() -> Self {
        Psg {
            registers: PlainRegisters::new(REGISTERS_START),
            wave_banks: [0; 32],
            tone_channels: [ToneChannel::default(), ToneChannel::default()],
            wave: WaveChannel::default(),
            noise: NoiseChannel::default(),
            frame_step: 0,
            frame_countdown: FRAME_SEQUENCER_PERIOD,
        }
    }

    fn register(&self, offset: u32) -> u16 {
        self.registers.read(offset)
    }

    /// Advance every channel by `cycles` CPU cycles (length, envelope, sweep, frequency timers).
    pub fn step(&mut self, mut cycles: u32) {
        while cycles > 0 {
            let chunk = cycles.min(self.frame_countdown);
            self.advance_frequency_timers(chunk);
            cycles -= chunk;
            self.frame_countdown -= chunk;
            if self.frame_countdown == 0 {
                self.frame_countdown = FRAME_SEQUENCER_PERIOD;
                self.tick_frame_sequencer();
            }
        }
    }

    fn advance_frequency_timers(&mut self, cycles: u32) {
        for tone in &mut self.tone_channels {
            tone.advance(cycles);
        }
        let digit_count = if self.register(SOUND3CNT_L) & 0x0020 != 0 { 64 } else { 32 };
        self.wave.advance(cycles, digit_count);
        let noise_control = self.register(SOUND4CNT_H);
        self.noise.advance(cycles, noise_control);
    }

    /// 512 Hz frame sequencer: length at 256 Hz, sweep at 128 Hz, envelope at 64 Hz.
    fn tick_frame_sequencer(&mut self) {
        let step = self.frame_step;
        self.frame_step = (step + 1) % 8;
        if step % 2 == 0 {
            self.tick_lengths();
        }
        if step == 2 || step == 6 {
            let sweep_control = self.register(SOUND1CNT_L);
            self.tone_channels[0].tick_sweep(sweep_control);
        }
        if step == 7 {
            let tone1_envelope = self.register(SOUND1CNT_H);
            let tone2_envelope = self.register(SOUND2CNT_L);
            let noise_envelope = self.register(SOUND4CNT_L);
            self.tone_channels[0].envelope.tick(tone1_envelope);
            self.tone_channels[1].envelope.tick(tone2_envelope);
            self.noise.envelope.tick(noise_envelope);
        }
    }

    fn tick_lengths(&mut self) {
        for tone in &mut self.tone_channels {
            if tone.length.tick() {
                tone.playing = false;
            }
        }
        if self.wave.length.tick() {
            self.wave.playing = false;
        }
        if self.noise.length.tick() {
            self.noise.playing = false;
        }
    }

    /// Current output of channels 1-4 as signed levels in -15..=15 (envelope or wave volume
    /// applied, 0 when the channel is off). The mixer scales and pans them.
    pub fn channel_outputs(&self) -> [i16; 4] {
        let noise = if !self.noise.playing {
            0
        } else if self.noise.output_high {
            self.noise.envelope.volume as i16
        } else {
            -(self.noise.envelope.volume as i16)
        };
        [
            self.tone_channels[0].output(self.register(SOUND1CNT_H)),
            self.tone_channels[1].output(self.register(SOUND2CNT_L)),
            self.wave_output(),
            noise,
        ]
    }

    /// Current wave digit mapped from 0..15 to -15..=15, scaled by SOUND3CNT_H volume.
    fn wave_output(&self) -> i16 {
        if !self.wave.playing {
            return 0;
        }
        let select = self.register(SOUND3CNT_L);
        let playing_bank = ((select >> 6) & 1) as usize;
        // In two-bank mode positions 32-63 play the other bank (index wraps over both banks).
        let digit_index = (playing_bank * 32 + self.wave.position as usize) % 64;
        let byte = self.wave_banks[digit_index / 2];
        let digit = if digit_index % 2 == 0 { byte >> 4 } else { byte & 0xF };
        let level = digit as i16 * 2 - 15;
        let volume = self.register(SOUND3CNT_H);
        if volume & 0x8000 != 0 {
            return level * 3 / 4;
        }
        match (volume >> 13) & 3 {
            0 => 0,
            1 => level,
            2 => level / 2,
            _ => level / 4,
        }
    }

    /// SOUNDCNT_X bits 0-3: which channels are currently playing.
    pub fn active_channels(&self) -> u8 {
        (self.tone_channels[0].playing as u8)
            | (self.tone_channels[1].playing as u8) << 1
            | (self.wave.playing as u8) << 2
            | (self.noise.playing as u8) << 3
    }

    #[allow(dead_code)] // called by the mixer (Phase 4 fifo-mixer worker)
    /// SOUNDCNT_X master enable cleared: reset the PSG registers (gbatek: SOUNDCNT_X bit 7).
    /// Wave RAM lies outside 0x060-0x081 and is kept.
    pub fn power_off(&mut self) {
        let wave_banks = self.wave_banks;
        *self = Psg::new();
        self.wave_banks = wave_banks;
    }

    /// Byte index into `wave_banks` for a CPU access: the bank that is not playing.
    fn wave_ram_index(&self, offset: u32) -> usize {
        let cpu_bank = (!self.register(SOUND3CNT_L) >> 6) as usize & 1;
        cpu_bank * 16 + (offset - WAVE_RAM_START) as usize
    }

    /// Offsets in 0x060-0x07F or 0x090-0x09F.
    pub fn io_read16(&mut self, offset: u32) -> u16 {
        if offset >= WAVE_RAM_START {
            let index = self.wave_ram_index(offset);
            return u16::from_le_bytes([self.wave_banks[index], self.wave_banks[index + 1]]);
        }
        self.registers.read(offset)
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        if offset >= WAVE_RAM_START {
            let index = self.wave_ram_index(offset);
            if mask & 0x00FF != 0 {
                self.wave_banks[index] = value as u8;
            }
            if mask & 0xFF00 != 0 {
                self.wave_banks[index + 1] = (value >> 8) as u8;
            }
            return;
        }
        self.registers.write(offset, value, mask);
        let low_byte_written = mask & 0x00FF != 0;
        let triggered = mask & TRIGGER_BIT != 0 && value & TRIGGER_BIT != 0;
        match offset {
            SOUND1CNT_H | SOUND2CNT_L => self.write_tone_duty_envelope(offset, value, low_byte_written),
            SOUND1CNT_X => self.write_tone_control(0, value, mask, triggered),
            SOUND2CNT_H => self.write_tone_control(1, value, mask, triggered),
            SOUND3CNT_L => {
                if value & 0x0080 == 0 && mask & 0x00FF != 0 {
                    self.wave.playing = false;
                }
            }
            SOUND3CNT_H if low_byte_written => self.wave.length.remaining = 256 - (value & 0xFF),
            SOUND3CNT_X => self.write_wave_control(value, mask, triggered),
            SOUND4CNT_L => {
                if low_byte_written {
                    self.noise.length.remaining = 64 - (value & 0x3F);
                }
                if envelope_silences_channel(value) && mask & 0xFF00 != 0 {
                    self.noise.playing = false;
                }
            }
            SOUND4CNT_H => {
                if mask & 0xFF00 != 0 {
                    self.noise.length.enabled = value & LENGTH_ENABLE_BIT != 0;
                }
                if triggered {
                    let envelope_control = self.register(SOUND4CNT_L);
                    self.noise.restart(envelope_control, value);
                }
            }
            _ => {}
        }
    }

    /// NRx1/NRx2: length load (low byte) and envelope; a silent envelope switches the channel off.
    fn write_tone_duty_envelope(&mut self, offset: u32, value: u16, low_byte_written: bool) {
        let tone = &mut self.tone_channels[if offset == SOUND1CNT_H { 0 } else { 1 }];
        if low_byte_written {
            tone.length.remaining = 64 - (value & 0x3F);
        }
        if envelope_silences_channel(self.registers.read(offset)) {
            tone.playing = false;
        }
    }

    /// NRx3/NRx4: frequency, length flag and restart for channel 1 or 2.
    fn write_tone_control(&mut self, index: usize, value: u16, mask: u16, triggered: bool) {
        let duty_envelope = self.register(if index == 0 { SOUND1CNT_H } else { SOUND2CNT_L });
        let sweep_control = self.register(SOUND1CNT_L);
        let tone = &mut self.tone_channels[index];
        tone.frequency = (tone.frequency & !mask) | (value & mask & 0x07FF);
        if mask & 0xFF00 != 0 {
            tone.length.enabled = value & LENGTH_ENABLE_BIT != 0;
        }
        if triggered {
            tone.restart(duty_envelope);
            if index == 0 {
                tone.restart_sweep(sweep_control);
            }
        }
    }

    /// NR33/NR34: sample rate, length flag and restart for channel 3.
    fn write_wave_control(&mut self, value: u16, mask: u16, triggered: bool) {
        let wave = &mut self.wave;
        wave.frequency = (wave.frequency & !mask) | (value & mask & 0x07FF);
        if mask & 0xFF00 != 0 {
            wave.length.enabled = value & LENGTH_ENABLE_BIT != 0;
        }
        if triggered {
            wave.playing = self.registers.read(SOUND3CNT_L) & 0x0080 != 0;
            wave.length.reload_if_expired(256);
            wave.position = 0;
            wave.countdown = wave.digit_period();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CYCLES_PER_SECOND: u32 = 16 * 1024 * 1024;

    #[test]
    fn tone_plays_duty_and_stops_on_length() {
        let mut psg = Psg::new();
        psg.io_write16(SOUND2CNT_L, 0xF080 | 0x30, 0xFFFF); // volume 15, 50% duty, length 16/256 s
        psg.io_write16(SOUND2CNT_H, TRIGGER_BIT | LENGTH_ENABLE_BIT | 1024, 0xFFFF);
        assert_eq!(psg.active_channels(), 0b0010);
        assert_eq!(psg.channel_outputs()[1], 15);
        psg.step(4 * 16 * 1024); // half a period at frequency 1024
        assert_eq!(psg.channel_outputs()[1], -15);
        psg.step(CYCLES_PER_SECOND / 16);
        assert_eq!(psg.active_channels(), 0);
        assert_eq!(psg.channel_outputs()[1], 0);
    }

    #[test]
    fn envelope_decreases_volume() {
        let mut psg = Psg::new();
        psg.io_write16(SOUND1CNT_H, 0xF180, 0xFFFF); // volume 15, decrease every 1/64 s
        psg.io_write16(SOUND1CNT_X, TRIGGER_BIT, 0xFFFF);
        psg.step(CYCLES_PER_SECOND / 64 * 5);
        assert_eq!(psg.tone_channels[0].envelope.volume, 10);
    }

    #[test]
    fn sweep_overflow_disables_channel() {
        let mut psg = Psg::new();
        psg.io_write16(SOUND1CNT_L, 0x0011, 0xFFFF); // time 1, increase, shift 1
        psg.io_write16(SOUND1CNT_H, 0xF000, 0xFFFF);
        psg.io_write16(SOUND1CNT_X, TRIGGER_BIT | 1200, 0xFFFF);
        assert_eq!(psg.active_channels(), 1);
        psg.step(CYCLES_PER_SECOND / 64);
        assert_eq!(psg.active_channels(), 0);
    }

    #[test]
    fn wave_reads_other_bank_and_plays_selected() {
        let mut psg = Psg::new();
        psg.io_write16(SOUND3CNT_L, 0x0040, 0xFFFF); // CPU sees bank 0
        psg.io_write16(WAVE_RAM_START, 0x00F0, 0xFFFF);
        psg.io_write16(SOUND3CNT_L, 0x0080, 0xFFFF); // play bank 0, CPU sees bank 1
        assert_eq!(psg.io_read16(WAVE_RAM_START), 0);
        psg.io_write16(SOUND3CNT_H, 0x2000, 0xFFFF); // 100% volume
        psg.io_write16(SOUND3CNT_X, TRIGGER_BIT, 0xFFFF);
        assert_eq!(psg.channel_outputs()[2], 15);
        psg.step(2048 * 8);
        assert_eq!(psg.channel_outputs()[2], -15);
    }

    #[test]
    fn noise_seven_bit_repeats_after_127_steps() {
        let mut psg = Psg::new();
        psg.io_write16(SOUND4CNT_L, 0xF000, 0xFFFF);
        psg.io_write16(SOUND4CNT_H, TRIGGER_BIT | 0x0008, 0xFFFF);
        let start = psg.noise.shift_register;
        psg.step(noise_period(0x0008) * 127);
        assert_eq!(psg.noise.shift_register, start);
        assert_eq!(psg.active_channels(), 0b1000);
    }
}
