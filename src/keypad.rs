//! Keypad input, IO 0x130-0x133 (gbatek: GBA Keypad Input).

use crate::io::merge;

const KEYINPUT: u32 = 0x130; // key status, active low
const KEYCNT: u32 = 0x132; // key interrupt control

/// The 10 key bits (A, B, Select, Start, Right, Left, Up, Down, R, L).
const KEY_BITS: u16 = 0x03FF;
const KEYCNT_IRQ_ENABLE: u16 = 1 << 14;
const KEYCNT_IRQ_CONDITION_AND: u16 = 1 << 15; // 0 = any selected key, 1 = all selected keys

pub struct Keypad {
    /// Pressed keys, active high, in ABI bit order (same order as KEYINPUT).
    pressed_keys: u16,
    key_control: u16,
}

impl Keypad {
    pub fn new() -> Self {
        Keypad { pressed_keys: 0, key_control: 0 }
    }

    /// Latch the host key state (active high); it holds until the next call.
    pub fn set_keys(&mut self, pressed_keys: u32) {
        self.pressed_keys = pressed_keys as u16 & KEY_BITS;
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        match offset {
            KEYINPUT => !self.pressed_keys & KEY_BITS,
            KEYCNT => self.key_control,
            _ => 0,
        }
    }

    /// Whether KEYCNT currently requests a keypad IRQ (gbatek: GBA Keypad Input, KEYCNT).
    /// The caller raises `IrqSource::Keypad` when this is true; the keypad holds no `Irq`.
    pub fn irq_condition_met(&self) -> bool {
        if self.key_control & KEYCNT_IRQ_ENABLE == 0 {
            return false;
        }
        let selected_keys = self.key_control & KEY_BITS;
        let selected_pressed = self.pressed_keys & selected_keys;
        if self.key_control & KEYCNT_IRQ_CONDITION_AND != 0 {
            selected_keys != 0 && selected_pressed == selected_keys
        } else {
            selected_pressed != 0
        }
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        // ponytail: raising the IRQ from irq_condition_met() needs wiring in gba.rs.
        if offset == KEYCNT {
            self.key_control = merge(self.key_control, value, mask);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyinput_is_active_low_and_keycnt_selects_irq_condition() {
        let mut keypad = Keypad::new();
        keypad.set_keys(0b11); // A and B pressed
        assert_eq!(keypad.io_read16(KEYINPUT), 0x03FC);

        keypad.io_write16(KEYCNT, KEYCNT_IRQ_ENABLE | 0b101, 0xFFFF); // A or Select
        assert!(keypad.irq_condition_met());
        keypad.io_write16(KEYCNT, KEYCNT_IRQ_ENABLE | KEYCNT_IRQ_CONDITION_AND | 0b101, 0xFFFF);
        assert!(!keypad.irq_condition_met());
        keypad.set_keys(0b101);
        assert!(keypad.irq_condition_met());
        keypad.io_write16(KEYCNT, 0b101, 0xFFFF); // IRQ disabled
        assert!(!keypad.irq_condition_met());
    }
}
