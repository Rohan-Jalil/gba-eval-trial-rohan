//! Keypad input, IO 0x130-0x133 (gbatek: GBA Keypad Input).

use crate::io::merge;

const KEYINPUT: u32 = 0x130; // key status, active low
const KEYCNT: u32 = 0x132; // key interrupt control

/// The 10 key bits (A, B, Select, Start, Right, Left, Up, Down, R, L).
const KEY_BITS: u16 = 0x03FF;

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

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        // ponytail: KEYCNT is stored but the keypad IRQ is not raised yet.
        if offset == KEYCNT {
            self.key_control = merge(self.key_control, value, mask);
        }
    }
}
