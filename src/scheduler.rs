//! Global cycle counter. Components are stepped by "cycles elapsed" from gba.rs; this only
//! keeps the running total so anything can ask "what time is it".

pub struct Scheduler {
    /// CPU cycles (16.78 MHz) since reset.
    pub total_cycles: u64,
}

impl Scheduler {
    pub fn new() -> Self {
        Scheduler { total_cycles: 0 }
    }

    pub fn advance(&mut self, cycles: u32) {
        self.total_cycles += cycles as u64;
    }
}
