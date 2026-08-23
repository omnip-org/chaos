use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug)]
pub struct Lifecycle {
    accepting_traffic: Arc<AtomicBool>,
}

impl Lifecycle {
    pub fn new() -> Self {
        Self {
            accepting_traffic: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn begin_draining(&self) {
        self.accepting_traffic.store(false, Ordering::Release);
    }

    pub fn is_accepting_traffic(&self) -> bool {
        self.accepting_traffic.load(Ordering::Acquire)
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}
