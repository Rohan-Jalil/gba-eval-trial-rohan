//! Direct sound channels A and B, fed through 32-byte FIFOs (gbatek: GBA Sound Channel A and B - DMA Sound).
//!
//! Phase 0 stub; filled in by the Phase 4 FIFO agent.

pub struct SoundFifo {}

impl SoundFifo {
    pub fn new() -> Self {
        SoundFifo {}
    }
}
