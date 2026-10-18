// SPDX-License-Identifier: BSD-3-Clause
//! Interrupt control (`metal_irq_*`).
//!
//! OpenAMP itself never registers interrupt handlers: the application's
//! handler for the IPI/mailbox interrupt calls into the rpmsg device (or just
//! wakes a task). This module only defines the controller interface and a
//! dispatch table for platforms that want libmetal-style registration.

use core::sync::atomic::Ordering;

#[cfg(not(loom))]
use core::{
    hint::spin_loop,
    sync::atomic::{AtomicPtr, AtomicUsize, fence},
};
#[cfg(loom)]
use loom::{
    hint::spin_loop,
    sync::atomic::{AtomicPtr, AtomicUsize, fence},
};

/// Enables and disables interrupt lines.
pub trait IrqController {
    /// Unmasks interrupt `irq`.
    fn enable(&self, irq: u32);
    /// Masks interrupt `irq`.
    fn disable(&self, irq: u32);
}

/// An interrupt handler: receives the irq number and its registered argument.
pub type IrqHandler = fn(irq: u32, arg: *mut ());

/// A fixed-size table mapping interrupt numbers to handlers.
///
/// Registration and dispatch are lock-free, so [`IrqTable::dispatch`] may be
/// called from an interrupt handler while thread code registers. A dispatch
/// that overlaps a registration of the same irq calls the old handler with
/// its argument, the new handler with its argument, or nothing; never a mix.
///
/// Registrations of the same irq are serialised, so do not register from an
/// interrupt handler an irq that the interrupted code may be registering.
pub struct IrqTable<const N: usize> {
    /// Per entry: odd while a registration is writing it (a sequence lock).
    seq: [AtomicUsize; N],
    handlers: [AtomicPtr<()>; N],
    args: [AtomicPtr<()>; N],
}

impl<const N: usize> IrqTable<N> {
    /// An empty table.
    #[cfg(not(loom))]
    pub const fn new() -> Self {
        IrqTable {
            seq: [const { AtomicUsize::new(0) }; N],
            handlers: [const { AtomicPtr::new(core::ptr::null_mut()) }; N],
            args: [const { AtomicPtr::new(core::ptr::null_mut()) }; N],
        }
    }

    /// An empty table (not `const` under loom).
    #[cfg(loom)]
    pub fn new() -> Self {
        IrqTable {
            seq: core::array::from_fn(|_| AtomicUsize::new(0)),
            handlers: core::array::from_fn(|_| AtomicPtr::new(core::ptr::null_mut())),
            args: core::array::from_fn(|_| AtomicPtr::new(core::ptr::null_mut())),
        }
    }

    /// Runs `write` on entry `i` with the sequence odd, so that readers
    /// retry or skip.
    fn write_entry(&self, i: usize, write: impl FnOnce()) {
        let seq = &self.seq[i];
        let mut s = seq.load(Ordering::Relaxed);
        loop {
            if s & 1 != 0 {
                // Another registration of this irq is writing.
                spin_loop();
                s = seq.load(Ordering::Relaxed);
                continue;
            }
            match seq.compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => break,
                Err(now) => s = now,
            }
        }
        // Orders the odd sequence before the entry stores: a reader that
        // sees any of them also sees the sequence change.
        fence(Ordering::Release);
        write();
        seq.store(s.wrapping_add(2), Ordering::Release);
    }

    /// Registers `handler` for `irq`, replacing any previous one.
    pub fn register(&self, irq: u32, handler: IrqHandler, arg: *mut ()) -> crate::Result<()> {
        let i = irq as usize;
        if i >= N {
            return Err(crate::Error::InvalidParam);
        }
        self.write_entry(i, || {
            self.handlers[i].store(handler as *mut (), Ordering::Relaxed);
            self.args[i].store(arg, Ordering::Relaxed);
        });
        Ok(())
    }

    /// Removes the handler for `irq`.
    pub fn unregister(&self, irq: u32) {
        let i = irq as usize;
        if i < N {
            self.write_entry(i, || {
                self.handlers[i].store(core::ptr::null_mut(), Ordering::Relaxed)
            });
        }
    }

    /// Calls the handler for `irq`; returns whether one was registered.
    ///
    /// Never waits: if a registration of `irq` is in progress, the irq is
    /// treated as having no handler.
    pub fn dispatch(&self, irq: u32) -> bool {
        let i = irq as usize;
        let Some(seq) = self.seq.get(i) else {
            return false;
        };
        let (f, arg) = loop {
            let s = seq.load(Ordering::Acquire);
            if s & 1 != 0 {
                return false;
            }
            let f = self.handlers[i].load(Ordering::Relaxed);
            let arg = self.args[i].load(Ordering::Relaxed);
            // Orders the entry loads before the re-check.
            fence(Ordering::Acquire);
            if seq.load(Ordering::Relaxed) == s {
                break (f, arg);
            }
            // A registration completed meanwhile (on another core): retry.
        };
        if f.is_null() {
            return false;
        }
        // SAFETY: non-null values are only ever stored from an `IrqHandler`
        // in `register`, and the pointer keeps that function's provenance.
        let f: IrqHandler = unsafe { core::mem::transmute::<*mut (), IrqHandler>(f) };
        f(irq, arg);
        true
    }
}

impl<const N: usize> Default for IrqTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicU32;

    static HITS: AtomicU32 = AtomicU32::new(0);

    fn handler(irq: u32, arg: *mut ()) {
        assert_eq!(arg as usize, 0x55);
        HITS.fetch_add(irq, Ordering::Relaxed);
    }

    #[test]
    fn register_and_dispatch() {
        let t: IrqTable<4> = IrqTable::new();
        assert!(!t.dispatch(2));
        t.register(2, handler, core::ptr::without_provenance_mut(0x55))
            .unwrap();
        assert!(t.dispatch(2));
        assert_eq!(HITS.load(Ordering::Relaxed), 2);
        t.unregister(2);
        assert!(!t.dispatch(2));
        assert!(t.register(9, handler, core::ptr::null_mut()).is_err());
    }
}
