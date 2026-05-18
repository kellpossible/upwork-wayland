//! ext_idle_notify_v1 idle backend.
//!
//! The actual ext_idle_notification_v1 subscription lives in `bridge::wayland`,
//! which updates `last_active` on `resumed` events. This backend just reads it.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::IdleBackend;

pub struct ExtIdleBackend {
    last_active: Arc<Mutex<Instant>>,
}

impl ExtIdleBackend {
    pub fn new(last_active: Arc<Mutex<Instant>>) -> Self {
        Self { last_active }
    }
}

impl IdleBackend for ExtIdleBackend {
    fn idle_time_ms(&self) -> u64 {
        let last = *self.last_active.lock().unwrap();
        Instant::now()
            .saturating_duration_since(last)
            .as_millis() as u64
    }
}
