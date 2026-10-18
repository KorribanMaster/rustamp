// SPDX-License-Identifier: BSD-3-Clause
//! Locking (`metal_mutex`, `metal_spinlock`) and a waker slot for async code.
//!
//! [`Mutex`] is generic over a [`RawLock`], so a platform can supply the
//! primitive that fits its execution model:
//!
//! * [`RawSpinLock`] (default): correct for multi-threaded code and for
//!   single-core firmware **as long as the lock is never taken from an
//!   interrupt handler**. In OpenAMP firmware the interrupt handler should
//!   only record or wake; rpmsg processing runs in thread context.
//! * A critical-section or RTOS mutex: implement [`RawLock`] for it.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::task::Waker;

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

const WAITING: usize = 0;
const REGISTERING: usize = 0b01;
const WAKING: usize = 0b10;

/// A slot holding one [`Waker`], safe to wake from an interrupt handler.
///
/// The algorithm is the one used by `futures::task::AtomicWaker`: `register`
/// and `wake` never block each other, and a wake that races with a register
/// is not lost.
pub struct AtomicWaker {
    state: AtomicUsize,
    waker: UnsafeCell<Option<Waker>>,
}

// SAFETY: access to `waker` is serialised by the state machine below.
unsafe impl Send for AtomicWaker {}
// SAFETY: as above.
unsafe impl Sync for AtomicWaker {}

impl AtomicWaker {
    /// An empty slot.
    pub const fn new() -> Self {
        AtomicWaker {
            state: AtomicUsize::new(WAITING),
            waker: UnsafeCell::new(None),
        }
    }

    /// Stores `waker`, to be woken by the next [`wake`](Self::wake).
    pub fn register(&self, waker: &Waker) {
        match self
            .state
            .compare_exchange(WAITING, REGISTERING, Ordering::Acquire, Ordering::Acquire)
            .unwrap_or_else(|x| x)
        {
            WAITING => {
                // SAFETY: the REGISTERING bit gives exclusive access.
                unsafe {
                    let slot = &mut *self.waker.get();
                    match slot {
                        Some(old) if old.will_wake(waker) => {}
                        _ => *slot = Some(waker.clone()),
                    }
                }
                let res = self.state.compare_exchange(
                    REGISTERING,
                    WAITING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                if res.is_err() {
                    // A wake arrived while registering: take and wake now.
                    // SAFETY: still holding the REGISTERING bit.
                    let w = unsafe { (*self.waker.get()).take() };
                    self.state.swap(WAITING, Ordering::AcqRel);
                    if let Some(w) = w {
                        w.wake();
                    }
                }
            }
            WAKING => waker.wake_by_ref(),
            _ => {} // Concurrent register: one of them wins; both are pending.
        }
    }

    /// Wakes the stored waker, if any.
    pub fn wake(&self) {
        if let Some(w) = self.take() {
            w.wake();
        }
    }

    /// Takes the stored waker without waking it.
    pub fn take(&self) -> Option<Waker> {
        match self.state.fetch_or(WAKING, Ordering::AcqRel) {
            WAITING => {
                // SAFETY: the WAKING bit gives exclusive access.
                let w = unsafe { (*self.waker.get()).take() };
                self.state.fetch_and(!WAKING, Ordering::Release);
                w
            }
            _ => None,
        }
    }
}

impl Default for AtomicWaker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::task::Wake;

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

    struct Flag(std::sync::atomic::AtomicBool);
    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn atomic_waker_wakes_once() {
        let flag = Arc::new(Flag(AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let slot = AtomicWaker::new();
        slot.wake(); // nothing registered: no-op
        slot.register(&waker);
        assert!(!flag.0.load(Ordering::SeqCst));
        slot.wake();
        assert!(flag.0.load(Ordering::SeqCst));
        assert!(slot.take().is_none());
    }
}
