//! Direct sound channels A and B, fed through 32-byte FIFOs (gbatek: GBA Sound Channel A and B - DMA Sound).

/// FIFO capacity: 8 x 32 bits.
const CAPACITY: usize = 32;
/// A FIFO holding this many bytes or fewer asks DMA for 16 more.
const REFILL_THRESHOLD: usize = 16;

/// One 32-byte sample FIFO plus the sample currently sent to the mixer.
pub struct SoundFifo {
    bytes: [u8; CAPACITY],
    read_index: usize,
    length: usize,
    /// Last sample moved to the sound circuit; held until the next timer overflow.
    pub current_sample: i8,
}

impl SoundFifo {
    pub fn new() -> Self {
        SoundFifo { bytes: [0; CAPACITY], read_index: 0, length: 0, current_sample: 0 }
    }

    /// SOUNDCNT_H reset bit: empty the FIFO and silence its output.
    pub fn reset(&mut self) {
        *self = SoundFifo::new();
    }

    /// Append one sample byte; bytes written to a full FIFO are dropped.
    pub fn push(&mut self, byte: u8) {
        if self.length == CAPACITY {
            return;
        }
        self.bytes[(self.read_index + self.length) % CAPACITY] = byte;
        self.length += 1;
    }

    /// FIFO_A/FIFO_B halfword write: push the bytes selected by `mask`, low byte first.
    pub fn write16(&mut self, value: u16, mask: u16) {
        for byte_index in 0..2 {
            let shift = byte_index * 8;
            if (mask >> shift) & 0xFF != 0 {
                self.push((value >> shift) as u8);
            }
        }
    }

    /// Timer overflow: move the oldest byte to the output (an empty FIFO keeps the last sample).
    /// Returns true when the FIFO now needs a DMA refill.
    pub fn pop_sample(&mut self) -> bool {
        if self.length > 0 {
            self.current_sample = self.bytes[self.read_index] as i8;
            self.read_index = (self.read_index + 1) % CAPACITY;
            self.length -= 1;
        }
        self.length <= REFILL_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_bytes_in_order_and_requests_refill_at_half() {
        let mut fifo = SoundFifo::new();
        for word in 0..5u32 {
            let base = (word * 4) as u16;
            fifo.write16(base | (base + 1) << 8, 0xFFFF);
            fifo.write16((base + 2) | (base + 3) << 8, 0xFFFF);
        }
        assert_eq!(fifo.length, 20);
        for expected in 0..3 {
            assert!(!fifo.pop_sample());
            assert_eq!(fifo.current_sample, expected);
        }
        assert!(fifo.pop_sample()); // 16 left
        fifo.write16(0x0080, 0x00FF); // byte write
        assert_eq!(fifo.length, 17);
        fifo.reset();
        assert_eq!((fifo.length, fifo.current_sample), (0, 0));
        fifo.write16(0x0080, 0x00FF);
        fifo.pop_sample();
        assert_eq!(fifo.current_sample, -128);
    }
}
