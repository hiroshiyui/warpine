// SPDX-License-Identifier: GPL-3.0-only

use std::sync::Mutex;

/// Extension trait for Mutex that recovers from poisoned locks instead of panicking.
/// If a thread panics while holding a lock, the data is still accessible.
pub trait MutexExt<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_or_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}
