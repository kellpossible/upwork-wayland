//! Idle backend trait.

pub mod ext;

pub trait IdleBackend: Send + Sync {
    /// Milliseconds since the last user activity.
    fn idle_time_ms(&self) -> u64;
}
