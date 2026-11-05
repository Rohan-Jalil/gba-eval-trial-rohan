//! Game Pak: ROM, save memory and GPIO (gbatek: GBA Cartridges, GBA Memory Map "Game Pak").
//!
//! Phase 0 stub: ROM reads and a plain 64 KiB SRAM. Flash/EEPROM detection and GPIO come in Phase 5.

/// Largest ROM the 0x08000000-0x09FFFFFF window can address.
pub const MAX_ROM_SIZE: usize = 32 * 1024 * 1024;
const SRAM_SIZE: usize = 0x10000;

pub struct Cart {
    rom: Vec<u8>,
    sram: Vec<u8>,
}

impl Cart {
    pub fn new(rom: &[u8]) -> Self {
        Cart {
            rom: rom[..rom.len().min(MAX_ROM_SIZE)].to_vec(),
            sram: vec![0xFF; SRAM_SIZE],
        }
    }

    fn rom_offset(address: u32) -> usize {
        (address as usize) & (MAX_ROM_SIZE - 1)
    }

    pub fn rom_read8(&self, address: u32) -> u8 {
        // ponytail: reads past the ROM end return 0, not the address-derived open-bus value.
        self.rom.get(Self::rom_offset(address)).copied().unwrap_or(0)
    }

    pub fn rom_read16(&self, address: u32) -> u16 {
        let offset = Self::rom_offset(address & !1);
        u16::from_le_bytes([
            self.rom.get(offset).copied().unwrap_or(0),
            self.rom.get(offset + 1).copied().unwrap_or(0),
        ])
    }

    /// Writes into the ROM window (GPIO at 0x080000C4-0x080000C9, EEPROM). Ignored for now.
    pub fn rom_write16(&mut self, _address: u32, _value: u16) {}

    pub fn sram_read8(&self, address: u32) -> u8 {
        self.sram[(address as usize) & (SRAM_SIZE - 1)]
    }

    pub fn sram_write8(&mut self, address: u32, value: u8) {
        self.sram[(address as usize) & (SRAM_SIZE - 1)] = value;
    }
}
