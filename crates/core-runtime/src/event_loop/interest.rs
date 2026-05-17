// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Interest tracking for the event loop.
//!
//! An [`InterestTracker`] counts external "keep-alive" requests — things
//! that should prevent the event loop from exiting even when no tasks are
//! queued. Builtins like `fetch` call [`acquire`](InterestTracker::acquire)
//! when they start an operation and [`release`](InterestTracker::release)
//! when it finishes.
//!
//! The event loop is considered alive when **either** the task queue has
//! pending work **or** the interest count is positive.

/// Tracks external keep-alive interest in the event loop.
///
/// The counter starts at zero. `acquire` increments it and `release`
/// decrements it. The event loop should keep running as long as
/// `has_interest()` returns true (or there are pending tasks).
///
/// # Panics
///
/// `release` panics if the counter would underflow,
/// which indicates a mismatched acquire/release pair.
#[derive(Debug, Default)]
pub struct InterestTracker {
    count: u32,
}

impl InterestTracker {
    /// Create a new tracker with zero interest.
    pub fn new() -> Self {
        Self { count: 0 }
    }

    /// Register interest — the event loop should stay alive.
    pub fn acquire(&mut self) {
        self.count = self.count.checked_add(1).expect("interest count overflow");
    }

    /// Release interest previously registered with [`acquire`](Self::acquire).
    ///
    /// # Panics
    ///
    /// Panics if no matching `acquire` was called (counter would underflow).
    pub fn release(&mut self) {
        self.count = self
            .count
            .checked_sub(1)
            .expect("InterestTracker::release called without matching acquire");
    }

    /// Returns `true` if at least one interest is held.
    pub fn has_interest(&self) -> bool {
        self.count > 0
    }

    /// Returns the current interest count.
    pub fn count(&self) -> u32 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero() {
        let tracker = InterestTracker::new();
        assert!(!tracker.has_interest());
        assert_eq!(tracker.count(), 0);
    }

    #[test]
    fn acquire_and_release() {
        let mut tracker = InterestTracker::new();
        tracker.acquire();
        assert!(tracker.has_interest());
        assert_eq!(tracker.count(), 1);

        tracker.acquire();
        assert_eq!(tracker.count(), 2);

        tracker.release();
        assert_eq!(tracker.count(), 1);
        assert!(tracker.has_interest());

        tracker.release();
        assert_eq!(tracker.count(), 0);
        assert!(!tracker.has_interest());
    }

    #[test]
    #[should_panic(expected = "without matching acquire")]
    fn release_underflow_panics() {
        let mut tracker = InterestTracker::new();
        tracker.release();
    }

    #[test]
    fn default_is_zero() {
        let tracker = InterestTracker::default();
        assert!(!tracker.has_interest());
    }
}
