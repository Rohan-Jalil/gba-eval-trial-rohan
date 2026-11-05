//! Game Pak: ROM, save memory and GPIO (gbatek: GBA Cartridges, GBA Memory Map "Game Pak").
//!
//! Phase 0 stub: ROM reads and a plain 64 KiB SRAM. Flash/EEPROM detection and GPIO come in Phase 5.

/// Largest ROM the 0x08000000-0x09FFFFFF window can address.
pub const MAX_ROM_SIZE: usize = 32 * 1024 * 1024;
/// Save area size when the ROM declares no backup type.
const DEFAULT_SAVE_SIZE: usize = 0x10000;
/// SRAM/FRAM carts hold 32 KiB (gbatek: GBA Cart Backup SRAM/FRAM), repeated twice in the 64K
/// save field (gbatek: GBA Memory Map, Memory Mirrors).
const SRAM_SIZE: usize = 0x8000;
/// Library ID string of SRAM carts (gbatek: GBA Cart Backup IDs).
const SRAM_ID: &[u8] = b"SRAM_V";

pub struct Cart {
    rom: Vec<u8>,
    sram: Vec<u8>,
    /// `sram.len() - 1`; the save window mirrors every `sram.len()` bytes.
    sram_mask: usize,
}

/// True when `rom` contains `id` at a word-aligned offset (gbatek: GBA Cart Backup IDs).
fn has_backup_id(rom: &[u8], id: &[u8]) -> bool {
    (0..rom.len()).step_by(4).any(|offset| rom[offset..].starts_with(id))
}

impl Cart {
    pub fn new(rom: &[u8]) -> Self {
        let rom = rom[..rom.len().min(MAX_ROM_SIZE)].to_vec();
        // ponytail: FLASH and EEPROM IDs fall back to plain 64 KiB SRAM; add those chips when a ROM needs them.
        let save_size = if has_backup_id(&rom, SRAM_ID) { SRAM_SIZE } else { DEFAULT_SAVE_SIZE };
        Cart { rom, sram: vec![0xFF; save_size], sram_mask: save_size - 1 }
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
        self.sram[(address as usize) & self.sram_mask]
    }

    pub fn sram_write8(&mut self, address: u32, value: u8) {
        self.sram[(address as usize) & self.sram_mask] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sram_id_selects_32k_mirrored_sram() {
        let mut rom = vec![0u8; 0x200];
        rom[0x100..0x10C].copy_from_slice(b"SRAM_V113\0\0\0");
        let mut cart = Cart::new(&rom);
        cart.sram_write8(0x0E00_8001, 0x42);
        assert_eq!(cart.sram_read8(0x0E00_0001), 0x42, "32K mirror");
        let mut plain = Cart::new(&[0; 0x200]);
        plain.sram_write8(0x0E00_8001, 0x42);
        assert_eq!(plain.sram_read8(0x0E00_0001), 0xFF, "64K without an ID");
    }
}
