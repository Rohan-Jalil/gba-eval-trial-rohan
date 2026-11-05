//! Native debug tool: run a ROM through the same ABI the grader uses, then dump the last frame.
//!
//!   cargo run --release --bin run_rom -- <rom.gba> <frames> <out.ppm> [out.wav]

use std::fs;
use std::process::exit;

use gba_emu::*;

const WIDTH: usize = 240;
const HEIGHT: usize = 160;

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.len() < 4 {
        eprintln!("usage: run_rom <rom.gba> <frames> <out.ppm> [out.wav]");
        exit(2);
    }
    let rom = fs::read(&arguments[1]).unwrap_or_else(|error| fail(&format!("read rom: {error}")));
    let frame_count: u32 = arguments[2].parse().unwrap_or_else(|_| fail("frames must be a number"));

    let mut audio: Vec<i16> = Vec::new();
    // SAFETY: the ABI hands out pointers into buffers that live for the whole program.
    unsafe {
        if emu_init() != 1 {
            fail("emu_init failed");
        }
        if rom.len() > 32 * 1024 * 1024 {
            fail("rom larger than 32 MiB");
        }
        std::ptr::copy_nonoverlapping(rom.as_ptr(), emu_rom_buffer(), rom.len());
        if emu_load_rom(rom.len() as i32) != 1 {
            fail("emu_load_rom failed");
        }
        for _ in 0..frame_count {
            emu_run_frame();
            let pairs = emu_audio_samples() as usize;
            audio.extend_from_slice(std::slice::from_raw_parts(emu_audio_buffer(), pairs * 2));
        }
        let pixels = std::slice::from_raw_parts(emu_framebuffer(), WIDTH * HEIGHT);
        write_ppm(&arguments[3], pixels);
    }
    if let Some(wav_path) = arguments.get(4) {
        write_wav(wav_path, &audio, emu_audio_rate() as u32);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("run_rom: {message}");
    exit(1);
}

/// Binary PPM (P6) from 0xAABBGGRR pixels.
fn write_ppm(path: &str, pixels: &[u32]) {
    let mut output = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    for &pixel in pixels {
        output.extend_from_slice(&[pixel as u8, (pixel >> 8) as u8, (pixel >> 16) as u8]);
    }
    fs::write(path, output).unwrap_or_else(|error| fail(&format!("write ppm: {error}")));
}

/// 16-bit stereo PCM WAV.
fn write_wav(path: &str, samples: &[i16], sample_rate: u32) {
    let data_bytes = (samples.len() * 2) as u32;
    let mut output = Vec::with_capacity(44 + data_bytes as usize);
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    output.extend_from_slice(b"WAVEfmt ");
    output.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    output.extend_from_slice(&1u16.to_le_bytes()); // PCM
    output.extend_from_slice(&2u16.to_le_bytes()); // channels
    output.extend_from_slice(&sample_rate.to_le_bytes());
    output.extend_from_slice(&(sample_rate * 4).to_le_bytes()); // byte rate
    output.extend_from_slice(&4u16.to_le_bytes()); // block align
    output.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    output.extend_from_slice(b"data");
    output.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        output.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, output).unwrap_or_else(|error| fail(&format!("write wav: {error}")));
}
