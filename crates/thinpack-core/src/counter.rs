use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug)]
pub struct IdCounter(AtomicU32);

impl Default for IdCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl IdCounter {
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicU32::new(1))
    }

    #[must_use]
    pub fn next(&self) -> u32 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}
