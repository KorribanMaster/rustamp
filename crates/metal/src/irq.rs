// SPDX-License-Identifier: BSD-3-Clause
//! Interrupt control (`metal_irq_*`).
//!
//! OpenAMP itself never registers interrupt handlers: the application's
//! handler for the IPI/mailbox interrupt calls into the rpmsg device (or just
//! wakes a task). This module only defines the controller interface and a
//! dispatch table for platforms that want libmetal-style registration.

use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

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
/// called from an interrupt handler while thread code registers.
pub struct IrqTable<const N: usize> {
    handlers: [AtomicUsize; N],
    args: [AtomicPtr<()>; N],
}

impl<const N: usize> IrqTable<N> {
    /// An empty table.
    pub const fn new() -> Self {
        IrqTable {
            handlers: [const { AtomicUsize::new(0) }; N],
            args: [const { AtomicPtr::new(core::ptr::null_mut()) }; N],
        }
    }

    /// Registers `handler` for `irq`, replacing any previous one.
    pub fn register(&self, irq: u32, handler: IrqHandler, arg: *mut ()) -> crate::Result<()> {
        let i = irq as usize;
        if i >= N {
            return Err(crate::Error::InvalidParam);
        }
        self.handlers[i].store(0, Ordering::Release);
        self.args[i].store(arg, Ordering::Release);
        self.handlers[i].store(handler as usize, Ordering::Release);
        Ok(())
    }

    /// Removes the handler for `irq`.
    pub fn unregister(&self, irq: u32) {
        if let Some(h) = self.handlers.get(irq as usize) {
            h.store(0, Ordering::Release);
        }
    }

    /// Calls the handler for `irq`; returns whether one was registered.
    pub fn dispatch(&self, irq: u32) -> bool {
        let i = irq as usize;
        let Some(h) = self.handlers.get(i) else {
            return false;
        };
        let f = h.load(Ordering::Acquire);
        if f == 0 {
            return false;
        }
        let arg = self.args[i].load(Ordering::Acquire);
        // SAFETY: non-zero values are only ever stored from an `IrqHandler`
        // in `register`.
        let f: IrqHandler = unsafe { core::mem::transmute::<usize, IrqHandler>(f) };
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
        t.register(2, handler, 0x55 as *mut ()).unwrap();
        assert!(t.dispatch(2));
        assert_eq!(HITS.load(Ordering::Relaxed), 2);
        t.unregister(2);
        assert!(!t.dispatch(2));
        assert!(t.register(9, handler, core::ptr::null_mut()).is_err());
    }
}
