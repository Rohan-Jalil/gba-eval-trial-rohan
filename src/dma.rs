//! Four DMA channels, IO 0x0B0-0x0DF (gbatek: GBA DMA Transfers).
//!
//! Each channel has 12 bytes of registers: SAD (source), DAD (destination), CNT_L (unit count)
//! and CNT_H (control). Start events only mark a channel as pending; `run_pending_transfers`,
//! called by gba.rs after every CPU step, then copies the data through the bus.
//! DMA lives inside the `Bus`, so the transfer itself is a free function taking the whole bus.

use crate::bus::Bus;
use crate::irq::IrqSource;

const IO_START: u32 = 0x0B0; // DMA0SAD, first DMA register
const IO_END: u32 = 0x0DF; // last byte of DMA3CNT_H
const CHANNEL_STRIDE: u32 = 12; // bytes of registers per channel

// Register offsets inside one channel's 12-byte block.
const SAD_LOW: u32 = 0x0; // source address, bits 0-15
const SAD_HIGH: u32 = 0x2; // source address, bits 16-31
const DAD_LOW: u32 = 0x4; // destination address, bits 0-15
const DAD_HIGH: u32 = 0x6; // destination address, bits 16-31
const CNT_L: u32 = 0x8; // word count
const CNT_H: u32 = 0xA; // control

// CNT_H fields.
const DESTINATION_CONTROL_SHIFT: u16 = 5; // bits 5-6
const SOURCE_CONTROL_SHIFT: u16 = 7; // bits 7-8
const REPEAT_BIT: u16 = 1 << 9;
const WORD_SIZE_BIT: u16 = 1 << 10; // 0 = 16-bit units, 1 = 32-bit units
const START_TIMING_SHIFT: u16 = 12; // bits 12-13
const IRQ_AT_END_BIT: u16 = 1 << 14;
const ENABLE_BIT: u16 = 1 << 15;

/// Sound FIFO destination addresses (gbatek: GBA Sound Channel A and B).
const FIFO_ADDRESSES: [u32; 2] = [0x0400_00A0, 0x0400_00A4];
/// A FIFO request always moves 4 words (gbatek: Sound DMA (FIFO Timing Mode)).
const FIFO_UNITS: u32 = 4;

const DMA_IRQ_SOURCES: [IrqSource; 4] = [IrqSource::Dma0, IrqSource::Dma1, IrqSource::Dma2, IrqSource::Dma3];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StartTiming {
    Immediate,
    VBlank,
    HBlank,
    Special, // FIFO on DMA1/DMA2, video capture on DMA3, prohibited on DMA0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AddressControl {
    Increment,
    Decrement,
    Fixed,
    IncrementReload, // destination only; for the source this setting is "prohibited"
}

fn address_control(bits: u16) -> AddressControl {
    match bits & 3 {
        0 => AddressControl::Increment,
        1 => AddressControl::Decrement,
        2 => AddressControl::Fixed,
        _ => AddressControl::IncrementReload,
    }
}

/// Move an internal address one unit according to its address control.
fn step_address(address: u32, control: AddressControl, unit_bytes: u32) -> u32 {
    match control {
        AddressControl::Increment | AddressControl::IncrementReload => address.wrapping_add(unit_bytes),
        AddressControl::Decrement => address.wrapping_sub(unit_bytes),
        AddressControl::Fixed => address,
    }
}

#[derive(Clone, Copy)]
struct Channel {
    index: usize,
    /// SAD/DAD/CNT_L as last written (write-only on hardware, never changed by transfers).
    source_register: u32,
    destination_register: u32,
    count_register: u16,
    control: u16,
    /// Internal pointers and counter, loaded on enable (and partly on repeat).
    internal_source: u32,
    internal_destination: u32,
    internal_count: u32,
    /// Start condition met; the transfer runs at the next `run_pending_transfers`.
    pending: bool,
}

impl Channel {
    fn new(index: usize) -> Self {
        Channel {
            index,
            source_register: 0,
            destination_register: 0,
            count_register: 0,
            control: 0,
            internal_source: 0,
            internal_destination: 0,
            internal_count: 0,
            pending: false,
        }
    }

    /// DMA0 sources and DMA0-2 destinations are limited to internal memory (27 bits).
    fn source_mask(&self) -> u32 {
        if self.index == 0 { 0x07FF_FFFF } else { 0x0FFF_FFFF }
    }

    fn destination_mask(&self) -> u32 {
        if self.index == 3 { 0x0FFF_FFFF } else { 0x07FF_FFFF }
    }

    /// Word count: 14 bits on DMA0-2, 16 bits on DMA3; 0 means the maximum.
    fn unit_count(&self) -> u32 {
        let (mask, maximum) = if self.index == 3 { (0xFFFF, 0x10000) } else { (0x3FFF, 0x4000) };
        match self.count_register as u32 & mask {
            0 => maximum,
            count => count,
        }
    }

    /// CNT_H bits that hold state: 0-4 are unused and bit 11 (Game Pak DRQ, never driven here)
    /// exists on DMA3 only.
    fn control_write_mask(&self) -> u16 {
        if self.index == 3 { 0xFFE0 } else { 0xF7E0 }
    }

    fn enabled(&self) -> bool {
        self.control & ENABLE_BIT != 0
    }

    fn start_timing(&self) -> StartTiming {
        match (self.control >> START_TIMING_SHIFT) & 3 {
            0 => StartTiming::Immediate,
            1 => StartTiming::VBlank,
            2 => StartTiming::HBlank,
            _ => StartTiming::Special,
        }
    }

    /// Sound FIFO mode: DMA1/DMA2 with special start timing.
    fn is_fifo_mode(&self) -> bool {
        (self.index == 1 || self.index == 2) && self.start_timing() == StartTiming::Special
    }

    /// Enable going 0 -> 1 copies SAD, DAD and CNT_L into the internal registers
    /// (gbatek: Source and Destination Address and Word Count Registers).
    fn latch(&mut self) {
        self.internal_source = self.source_register & self.source_mask();
        self.internal_destination = self.destination_register & self.destination_mask();
        self.internal_count = self.unit_count();
        self.pending = self.start_timing() == StartTiming::Immediate;
    }

    fn write_control(&mut self, value: u16, mask: u16) {
        let was_enabled = self.enabled();
        let writable = mask & self.control_write_mask();
        self.control = (self.control & !writable) | (value & writable);
        if !self.enabled() {
            self.pending = false;
        } else if !was_enabled {
            self.latch();
        }
    }

    /// Mark the channel pending if it is enabled and waiting for `timing`.
    fn trigger(&mut self, timing: StartTiming) {
        if self.enabled() && self.start_timing() == timing {
            self.pending = true;
        }
    }
}

pub struct Dma {
    channels: [Channel; 4],
}

impl Dma {
    pub fn new() -> Self {
        Dma { channels: [Channel::new(0), Channel::new(1), Channel::new(2), Channel::new(3)] }
    }

    /// PPU entered HBlank on a visible line (start-timing mode 2).
    pub fn notify_hblank(&mut self) {
        // ponytail: DMA3 video capture (special timing, lines 2-161) never starts; add when a ROM uses it.
        self.channels.iter_mut().for_each(|channel| channel.trigger(StartTiming::HBlank));
    }

    /// PPU entered VBlank (start-timing mode 1).
    pub fn notify_vblank(&mut self) {
        self.channels.iter_mut().for_each(|channel| channel.trigger(StartTiming::VBlank));
    }

    /// APU sound FIFO A (0) or B (1) wants more samples (start-timing mode 3 on DMA1/DMA2).
    /// Only a channel whose destination is that FIFO answers.
    pub fn notify_fifo_request(&mut self, fifo_index: usize) {
        for channel in &mut self.channels[1..=2] {
            if channel.enabled()
                && channel.is_fifo_mode()
                && channel.internal_destination == FIFO_ADDRESSES[fifo_index]
            {
                channel.pending = true;
            }
        }
    }

    /// SAD, DAD and CNT_L are write-only and read as 0; CNT_H reads back.
    // ponytail: write-only registers read 0, not open bus; the integrator's open-bus work may change it.
    pub fn io_read16(&mut self, offset: u32) -> u16 {
        let (channel_index, register) = split_offset(offset);
        match register {
            CNT_H => self.channels[channel_index].control,
            _ => 0,
        }
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        let (channel_index, register) = split_offset(offset);
        let channel = &mut self.channels[channel_index];
        match register {
            SAD_LOW => channel.source_register = merge_low(channel.source_register, value, mask),
            SAD_HIGH => channel.source_register = merge_high(channel.source_register, value, mask),
            DAD_LOW => channel.destination_register = merge_low(channel.destination_register, value, mask),
            DAD_HIGH => channel.destination_register = merge_high(channel.destination_register, value, mask),
            CNT_L => channel.count_register = crate::io::merge(channel.count_register, value, mask),
            _ => channel.write_control(value, mask),
        }
    }

    fn highest_priority_pending(&self) -> Option<usize> {
        self.channels.iter().position(|channel| channel.pending)
    }
}

/// Channel number and register offset within the channel for an IO offset in 0x0B0-0x0DF.
fn split_offset(offset: u32) -> (usize, u32) {
    debug_assert!((IO_START..=IO_END).contains(&offset));
    let relative = offset - IO_START;
    ((relative / CHANNEL_STRIDE) as usize, relative % CHANNEL_STRIDE)
}

fn merge_low(old: u32, value: u16, mask: u16) -> u32 {
    (old & 0xFFFF_0000) | crate::io::merge(old as u16, value, mask) as u32
}

fn merge_high(old: u32, value: u16, mask: u16) -> u32 {
    (old & 0x0000_FFFF) | (crate::io::merge((old >> 16) as u16, value, mask) as u32) << 16
}

fn is_game_pak(address: u32) -> bool {
    (0x0800_0000..0x1000_0000).contains(&address)
}

/// Run any DMA transfer that is due, using the bus for the copies.
/// Returns the cycles the transfers took (the CPU is stalled for that time).
/// Channels run in priority order, DMA0 first.
// ponytail: a transfer runs to completion in one go, so a start event that would happen in the
// middle of a long transfer (e.g. HBlank during a big DMA3) is only seen afterwards; true
// mid-transfer pre-emption needs gba.rs to advance the PPU between units.
pub fn run_pending_transfers(bus: &mut Bus) -> u32 {
    let mut total_cycles = 0;
    while let Some(channel_index) = bus.dma.highest_priority_pending() {
        total_cycles += run_transfer(bus, channel_index);
    }
    total_cycles
}

/// One complete transfer of a channel, then the end-of-transfer handling
/// (gbatek: DMA Repeat bit, Transfer End, Transfer Rate/Timing).
fn run_transfer(bus: &mut Bus, channel_index: usize) -> u32 {
    bus.dma.channels[channel_index].pending = false;
    let channel = bus.dma.channels[channel_index];

    let fifo_mode = channel.is_fifo_mode();
    let word_sized = fifo_mode || channel.control & WORD_SIZE_BIT != 0;
    let unit_bytes = if word_sized { 4 } else { 2 };
    let unit_count = if fifo_mode { FIFO_UNITS } else { channel.internal_count };
    let source_control = address_control(channel.control >> SOURCE_CONTROL_SHIFT);
    let destination_control = if fifo_mode {
        AddressControl::Fixed
    } else {
        address_control(channel.control >> DESTINATION_CONTROL_SHIFT)
    };

    // Internal processing time: 2I, or 4I when both ends are in the Game Pak.
    let both_in_game_pak = is_game_pak(channel.internal_source) && is_game_pak(channel.internal_destination);
    let internal_cycles = if both_in_game_pak { 4 } else { 2 };
    let cycles_before = bus.take_access_cycles();

    let mut source = channel.internal_source;
    let mut destination = channel.internal_destination;
    for _ in 0..unit_count {
        if word_sized {
            let value = bus.read32(source);
            bus.write32(destination, value);
        } else {
            let value = bus.read16(source);
            bus.write16(destination, value);
        }
        source = step_address(source, source_control, unit_bytes) & channel.source_mask();
        destination = step_address(destination, destination_control, unit_bytes) & channel.destination_mask();
    }
    // ponytail: every access is charged as non-sequential by the bus; gbatek's 2N+2(n-1)S needs an S/N split there.
    let transfer_cycles = bus.take_access_cycles() + internal_cycles;
    bus.access_cycles = cycles_before;

    // Re-read the live channel: the transfer itself may have written this channel's registers.
    let live = &mut bus.dma.channels[channel_index];
    live.internal_source = source;
    live.internal_destination = destination;
    let repeats = live.control & REPEAT_BIT != 0 && live.start_timing() != StartTiming::Immediate;
    if repeats {
        live.internal_count = live.unit_count();
        if destination_control == AddressControl::IncrementReload {
            live.internal_destination = live.destination_register & live.destination_mask();
        }
    } else {
        live.control &= !ENABLE_BIT;
        live.pending = false;
    }
    if channel.control & IRQ_AT_END_BIT != 0 {
        bus.irq.request(DMA_IRQ_SOURCES[channel_index]);
    }
    transfer_cycles
}

#[cfg(test)]
mod tests {
    use super::*;

    const DMA3: u32 = 0x0400_00D4;

    fn program_dma3(bus: &mut Bus, source: u32, destination: u32, count: u16, control: u16) {
        bus.write32(DMA3, source);
        bus.write32(DMA3 + 4, destination);
        bus.write16(DMA3 + 8, count);
        bus.write16(DMA3 + 10, control);
    }

    #[test]
    fn immediate_word_copy_then_disables_and_raises_irq() {
        let mut bus = Bus::new(&[]);
        for i in 0..4 {
            bus.write32(0x0200_0000 + i * 4, 0x1111_1111 * (i + 1));
        }
        program_dma3(&mut bus, 0x0200_0000, 0x0300_0100, 4, ENABLE_BIT | WORD_SIZE_BIT | IRQ_AT_END_BIT);
        bus.take_access_cycles();
        let cycles = run_pending_transfers(&mut bus);
        assert_eq!(bus.read32(0x0300_010C), 0x4444_4444);
        assert_eq!(cycles, 4 * (6 + 1) + 2);
        assert_eq!(bus.read16(DMA3 + 10) & ENABLE_BIT, 0);
        assert_ne!(bus.irq.interrupt_flags & (1 << 11), 0);
    }

    #[test]
    fn decrement_and_fixed_address_control() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0300_0004, 0xAAAA);
        bus.write16(0x0300_0002, 0xBBBB);
        // Source decrement (1 << 7), destination fixed (2 << 5), halfwords.
        program_dma3(&mut bus, 0x0300_0004, 0x0300_0200, 2, ENABLE_BIT | (1 << 7) | (2 << 5));
        run_pending_transfers(&mut bus);
        assert_eq!(bus.read16(0x0300_0200), 0xBBBB);
    }

    #[test]
    fn hblank_repeat_reloads_destination_and_waits_for_trigger() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0300_0000, 0x1234);
        let control = ENABLE_BIT | REPEAT_BIT | (2 << START_TIMING_SHIFT) | (3 << DESTINATION_CONTROL_SHIFT);
        program_dma3(&mut bus, 0x0300_0000, 0x0300_0100, 1, control);
        assert_eq!(run_pending_transfers(&mut bus), 0);
        bus.dma.notify_hblank();
        run_pending_transfers(&mut bus);
        assert_eq!(bus.read16(0x0300_0100), 0x1234);
        assert_eq!(bus.dma.channels[3].internal_destination, 0x0300_0100);
        assert_ne!(bus.read16(DMA3 + 10) & ENABLE_BIT, 0);
    }

    #[test]
    fn higher_priority_channel_runs_first() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0300_0000, 0x0001);
        bus.write16(0x0300_0002, 0x0002);
        let vblank = ENABLE_BIT | (1 << START_TIMING_SHIFT);
        // DMA3 and DMA0 both write the same destination; DMA0 must go first, so DMA3's value stays.
        program_dma3(&mut bus, 0x0300_0002, 0x0300_0100, 1, vblank);
        bus.write32(0x0400_00B0, 0x0300_0000);
        bus.write32(0x0400_00B4, 0x0300_0100);
        bus.write16(0x0400_00B8, 1);
        bus.write16(0x0400_00BA, vblank);
        bus.dma.notify_vblank();
        run_pending_transfers(&mut bus);
        assert_eq!(bus.read16(0x0300_0100), 0x0002);
    }

    #[test]
    fn fifo_request_moves_four_words_to_fixed_fifo() {
        let mut bus = Bus::new(&[]);
        bus.write32(0x0400_00BC, 0x0300_0000);
        bus.write32(0x0400_00C0, FIFO_ADDRESSES[0]);
        bus.write16(0x0400_00C6, ENABLE_BIT | REPEAT_BIT | (3 << START_TIMING_SHIFT));
        bus.dma.notify_fifo_request(1); // wrong FIFO: nothing happens
        assert_eq!(run_pending_transfers(&mut bus), 0);
        bus.dma.notify_fifo_request(0);
        run_pending_transfers(&mut bus);
        assert_eq!(bus.dma.channels[1].internal_source, 0x0300_0010);
        assert_eq!(bus.dma.channels[1].internal_destination, FIFO_ADDRESSES[0]);
    }

    #[test]
    fn zero_count_means_maximum_and_dma0_cannot_reach_game_pak() {
        let mut channel = Channel::new(0);
        assert_eq!(channel.unit_count(), 0x4000);
        channel.source_register = 0x0800_0000;
        channel.control = (1 << START_TIMING_SHIFT) | ENABLE_BIT;
        channel.latch();
        assert_eq!(channel.internal_source, 0);
        assert_eq!(Channel::new(3).unit_count(), 0x10000);
    }
}
