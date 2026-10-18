// SPDX-License-Identifier: BSD-3-Clause
//! Locking (`metal_mutex`, `metal_spinlock`).
//!
//! [`Mutex`] is generic over a [`RawLock`], so a platform can supply the
//! primitive that fits its execution model:
//!
//! * [`RawSpinLock`] (default): correct for multi-threaded code and for
//!   single-core firmware **as long as the lock is never taken from an
//!   interrupt handler**. In OpenAMP firmware the interrupt handler should
//!   only record or wake; rpmsg processing runs in thread context.
//! * `CsLock` (feature `critical-section`): masks interrupts while held, so
//!   it is correct when rpmsg is also used from an interrupt handler.
//! * An RTOS mutex: implement [`RawLock`] for it.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
#[cfg(feature = "critical-section")]
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::{AtomicBool, Ordering};

/// A raw mutual-exclusion primitive.
///
/// # Safety
///
/// Implementations must guarantee that between a `lock` (or successful
/// `try_lock`) and the matching `unlock`, no other `lock`/`try_lock` on the
/// same instance succeeds, and that `unlock` has release semantics and
/// `lock` acquire semantics.
pub unsafe trait RawLock {
    /// An unlocked instance.
    const INIT: Self;
    /// Acquires the lock, blocking or spinning.
    fn lock(&self);
    /// Attempts to acquire the lock without waiting.
    fn try_lock(&self) -> bool;
    /// Releases the lock.
    ///
    /// # Safety
    ///
    /// The lock must be held by the caller.
    unsafe fn unlock(&self);
}

/// A test-and-test-and-set spin lock.
#[derive(Debug)]
pub struct RawSpinLock(AtomicBool);

// SAFETY: compare_exchange(Acquire) only succeeds while the flag is false, and
// unlock stores false with Release.
unsafe impl RawLock for RawSpinLock {
    const INIT: Self = RawSpinLock(AtomicBool::new(false));

    fn lock(&self) {
        while !self.try_lock() {
            while self.0.load(Ordering::Relaxed) {
                core::hint::spin_loop();
                #[cfg(feature = "std")]
                std::thread::yield_now();
            }
        }
    }

    fn try_lock(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    unsafe fn unlock(&self) {
        self.0.store(false, Ordering::Release);
    }
}

/// A lock built on the [`critical-section`](https://docs.rs/critical-section)
/// crate.
///
/// While it is held, the platform's critical section is active: on
/// single-core firmware that means interrupts are masked, so an interrupt
/// handler can never find the lock taken. Every rpmsg operation then runs
/// with interrupts disabled, which is a few microseconds.
///
/// It is not the default: on `std` hosts and multi-core firmware the critical
/// section is one global lock, shared by every `CsLock` and by everything else
/// that uses `critical-section`.
///
/// Locks must be released in the reverse order they were taken, as
/// `critical-section` requires. Nested guards dropped at the end of their
/// scopes do this; releasing out of order panics.
#[cfg(feature = "critical-section")]
pub struct CsLock {
    held: AtomicBool,
    /// Nesting depth of the critical section when this lock was taken.
    depth: AtomicUsize,
    state: UnsafeCell<core::mem::MaybeUninit<critical_section::RestoreState>>,
}

/// Number of `CsLock`s currently held. Only changed inside the critical
/// section.
#[cfg(feature = "critical-section")]
static CS_DEPTH: AtomicUsize = AtomicUsize::new(0);

/// A mutex that masks interrupts while held.
#[cfg(feature = "critical-section")]
pub type CsMutex<T> = Mutex<CsLock, T>;

// SAFETY: `held` is only read and written inside the critical section, which
// excludes every other context, so at most one holder exists at a time.
// `acquire` and `release` give the critical section's acquire and release
// ordering.
#[cfg(feature = "critical-section")]
unsafe impl RawLock for CsLock {
    const INIT: Self = CsLock {
        held: AtomicBool::new(false),
        depth: AtomicUsize::new(0),
        state: UnsafeCell::new(core::mem::MaybeUninit::uninit()),
    };

    fn lock(&self) {
        while !self.try_lock() {
            core::hint::spin_loop();
        }
    }

    fn try_lock(&self) -> bool {
        // SAFETY: released below on contention, or by `unlock`, in nesting
        // order (checked there).
        let state = unsafe { critical_section::acquire() };
        if self.held.load(Ordering::Relaxed) {
            // Taken by an outer frame of the same context.
            // SAFETY: `state` comes from the `acquire` just above, and
            // nothing was acquired since.
            unsafe { critical_section::release(state) };
            return false;
        }
        self.held.store(true, Ordering::Relaxed);
        let depth = CS_DEPTH.load(Ordering::Relaxed) + 1;
        CS_DEPTH.store(depth, Ordering::Relaxed);
        self.depth.store(depth, Ordering::Relaxed);
        // SAFETY: we hold the lock, so nothing else touches `state`.
        unsafe { (*self.state.get()).write(state) };
        true
    }

    unsafe fn unlock(&self) {
        let depth = CS_DEPTH.load(Ordering::Relaxed);
        assert_eq!(
            self.depth.load(Ordering::Relaxed),
            depth,
            "CsLock released out of order"
        );
        CS_DEPTH.store(depth - 1, Ordering::Relaxed);
        self.held.store(false, Ordering::Relaxed);
        // SAFETY: the caller holds the lock, so `try_lock` wrote `state`, and
        // the depth check makes this the innermost critical section.
        unsafe { critical_section::release((*self.state.get()).assume_init()) };
    }
}

// SAFETY: all fields are only accessed by the lock holder, inside the
// critical section.
#[cfg(feature = "critical-section")]
unsafe impl Sync for CsLock {}

#[cfg(feature = "critical-section")]
impl core::fmt::Debug for CsLock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CsLock")
            .field("held", &self.held.load(Ordering::Relaxed))
            .finish()
    }
}

/// A mutex protecting `T` with the raw lock `R`.
pub struct Mutex<R: RawLock, T: ?Sized> {
    raw: R,
    data: UnsafeCell<T>,
}

/// A spin-lock mutex.
pub type SpinMutex<T> = Mutex<RawSpinLock, T>;

// SAFETY: the raw lock serialises all access to `data`.
unsafe impl<R: RawLock + Send, T: ?Sized + Send> Send for Mutex<R, T> {}
// SAFETY: the raw lock serialises all access to `data`.
unsafe impl<R: RawLock + Sync, T: ?Sized + Send> Sync for Mutex<R, T> {}

impl<R: RawLock, T> Mutex<R, T> {
    /// Creates an unlocked mutex.
    pub const fn new(value: T) -> Self {
        Mutex {
            raw: R::INIT,
            data: UnsafeCell::new(value),
        }
    }

    /// Consumes the mutex, returning the data.
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<R: RawLock, T: ?Sized> Mutex<R, T> {
    /// Locks the mutex.
    pub fn lock(&self) -> MutexGuard<'_, R, T> {
        self.raw.lock();
        MutexGuard { mutex: self }
    }

    /// Locks the mutex if it is free.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, R, T>> {
        self.raw.try_lock().then(|| MutexGuard { mutex: self })
    }

    /// Mutable access without locking (exclusive borrow proves no sharing).
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }
}

impl<R: RawLock, T: Default> Default for Mutex<R, T> {
    fn default() -> Self {
        Mutex::new(T::default())
    }
}

/// RAII guard returned by [`Mutex::lock`].
pub struct MutexGuard<'a, R: RawLock, T: ?Sized> {
    mutex: &'a Mutex<R, T>,
}

impl<R: RawLock, T: ?Sized> Deref for MutexGuard<'_, R, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves the lock is held.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<R: RawLock, T: ?Sized> DerefMut for MutexGuard<'_, R, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard proves the lock is held, and `&mut self` makes
        // this the only reference derived from it.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<R: RawLock, T: ?Sized> Drop for MutexGuard<'_, R, T> {
    fn drop(&mut self) {
        // SAFETY: the guard proves the lock is held by us.
        unsafe { self.mutex.raw.unlock() }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn mutex_serialises_threads() {
        let m: Arc<SpinMutex<u64>> = Arc::new(Mutex::new(0));
        let hs: std::vec::Vec<_> = (0..4)
            .map(|_| {
                let m = m.clone();
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        *m.lock() += 1;
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(*m.lock(), 40_000);
        assert!(m.try_lock().is_some());
    }

    #[test]
    fn failed_try_lock_keeps_lock_held() {
        let m: SpinMutex<u8> = Mutex::new(0);
        let _g = m.lock();
        assert!(m.try_lock().is_none());
        // A failed try_lock must not have released the holder's lock.
        assert!(m.try_lock().is_none());
    }

    #[cfg(feature = "critical-section")]
    #[test]
    fn cs_mutex_serialises_threads() {
        let m: Arc<CsMutex<u64>> = Arc::new(Mutex::new(0));
        let n = if cfg!(miri) { 50 } else { 10_000 };
        let hs: std::vec::Vec<_> = (0..4)
            .map(|_| {
                let m = m.clone();
                std::thread::spawn(move || {
                    for _ in 0..n {
                        *m.lock() += 1;
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(*m.lock(), 4 * n);
    }

    #[cfg(feature = "critical-section")]
    #[test]
    fn cs_mutex_nests_and_detects_recursion() {
        let a: CsMutex<u8> = Mutex::new(1);
        let b: CsMutex<u8> = Mutex::new(2);
        {
            let ga = a.lock();
            assert!(a.try_lock().is_none());
            let gb = b.lock();
            assert_eq!(*ga + *gb, 3);
        }
        assert!(a.try_lock().is_some());
    }
}
