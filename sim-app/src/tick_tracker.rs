use tracing::info;

#[derive(Default)]
pub struct TickTracker {
    last: u64,
}

impl TickTracker {
    pub fn new() -> Self {
        Self { last: 0 }
    }

    pub fn update_with(&mut self, app_id: &str, tick: u64) -> Option<u64> {
        if tick > self.last {
            info!("[Client: {app_id}] Updated to tick: {}", tick);
            self.last = tick;
            return Some(tick);
        }
        None
    }

    pub fn last(&self) -> u64 {
        self.last
    }
}
