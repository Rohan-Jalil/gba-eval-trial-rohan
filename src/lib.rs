//! gba_emu: an original Game Boy Advance emulator.
//!
//! This file only holds the 10 C-ABI exports from `gba-eval/spec/ABI.md`, as thin wrappers over a
//! single global `Gba`. The wasm build has no imports: no printing, no threads, no filesystem.

pub mod apu;
pub mod bus;
pub mod cart;
pub mod cpu;
pub mod dma;
pub mod gba;
pub mod io;
pub mod irq;
pub mod keypad;
pub mod ppu;
pub mod scheduler;
pub mod timer;

use std::ptr::addr_of_mut;

use cart::MAX_ROM_SIZE;
use gba::Gba;
use ppu::{SCREEN_HEIGHT, SCREEN_WIDTH};

const FRAMEBUFFER_PIXELS: usize = SCREEN_WIDTH * SCREEN_HEIGHT;
/// Room for several frames of stereo pairs at the highest rate (262144 Hz ~ 4390 pairs/frame).
const AUDIO_BUFFER_PAIRS: usize = 1 << 15;

/// Host-visible state. The buffers live here, outside `Gba`, so their pointers stay valid
/// across `emu_load_rom` / `emu_reset`.
struct Emulator {
    rom_buffer: Vec<u8>,
    rom_length: usize,
    gba: Option<Box<Gba>>,
    framebuffer: Vec<u32>,
    audio_buffer: Vec<i16>,
}

static mut EMULATOR: Option<Emulator> = None;

/// The global emulator, created on first use.
fn emulator() -> &'static mut Emulator {
    // SAFETY: the ABI is single-threaded (wasm, no threads) and no export re-enters another,
    // so there is never more than one live reference.
    unsafe {
        (*addr_of_mut!(EMULATOR)).get_or_insert_with(|| Emulator {
            rom_buffer: vec![0; MAX_ROM_SIZE],
            rom_length: 0,
            gba: None,
            framebuffer: vec![0xFF00_0000; FRAMEBUFFER_PIXELS],
            audio_buffer: vec![0; AUDIO_BUFFER_PAIRS * 2],
        })
    }
}

impl Emulator {
    /// Power on with the ROM currently in the buffer; the same path serves load and reset.
    fn power_on(&mut self) {
        self.gba = Some(Box::new(Gba::new(&self.rom_buffer[..self.rom_length])));
        self.framebuffer.fill(0xFF00_0000);
    }
}

/// Set up global state. Returns 1 on success.
#[no_mangle]
pub extern "C" fn emu_init() -> i32 {
    emulator();
    1
}

/// Stable pointer to a 32 MiB buffer the host copies the ROM into before `emu_load_rom`.
#[no_mangle]
pub extern "C" fn emu_rom_buffer() -> *mut u8 {
    emulator().rom_buffer.as_mut_ptr()
}

/// Load the first `length` bytes of the ROM buffer and reset. Returns 1 on success.
#[no_mangle]
pub extern "C" fn emu_load_rom(length: i32) -> i32 {
    if length <= 0 || length as usize > MAX_ROM_SIZE {
        return 0;
    }
    let emulator = emulator();
    emulator.rom_length = length as usize;
    emulator.power_on();
    1
}

/// Reset to exactly the state `emu_load_rom` produces. Returns 0 if no ROM is loaded.
#[no_mangle]
pub extern "C" fn emu_reset() -> i32 {
    let emulator = emulator();
    if emulator.rom_length == 0 {
        return 0;
    }
    emulator.power_on();
    1
}

/// Latch the pressed keys (active high, bit 0 = A ... bit 9 = L).
#[no_mangle]
pub extern "C" fn emu_set_keys(keys: u32) {
    if let Some(gba) = emulator().gba.as_mut() {
        gba.set_keys(keys);
    }
}

/// Advance one frame (280,896 cycles) and publish its picture.
#[no_mangle]
pub extern "C" fn emu_run_frame() {
    let emulator = emulator();
    if let Some(gba) = emulator.gba.as_mut() {
        gba.run_frame();
        emulator.framebuffer.copy_from_slice(&gba.bus.ppu.framebuffer);
    }
}

/// 240x160 pixels, 0xAABBGGRR with alpha 0xFF.
#[no_mangle]
pub extern "C" fn emu_framebuffer() -> *mut u32 {
    emulator().framebuffer.as_mut_ptr()
}

/// Interleaved stereo i16 samples; valid count is the last `emu_audio_samples()` result.
#[no_mangle]
pub extern "C" fn emu_audio_buffer() -> *mut i16 {
    emulator().audio_buffer.as_mut_ptr()
}

/// Move the samples produced since the last call into the audio buffer; returns stereo pairs.
#[no_mangle]
pub extern "C" fn emu_audio_samples() -> i32 {
    let emulator = emulator();
    let Some(gba) = emulator.gba.as_mut() else { return 0 };
    let produced = &mut gba.bus.apu.samples;
    let copied = produced.len().min(emulator.audio_buffer.len());
    emulator.audio_buffer[..copied].copy_from_slice(&produced[..copied]);
    produced.clear();
    (copied / 2) as i32
}

/// Current output sample rate in Hz (32768 << SOUNDBIAS resolution).
#[no_mangle]
pub extern "C" fn emu_audio_rate() -> i32 {
    match emulator().gba.as_ref() {
        Some(gba) => gba.bus.apu.sample_rate() as i32,
        None => 32768,
    }
}
