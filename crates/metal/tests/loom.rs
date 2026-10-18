// SPDX-License-Identifier: BSD-3-Clause
//! Model checks of the lock-free code with loom:
//!
//! ```sh
//! RUSTFLAGS="--cfg loom" cargo test -p rustamp-metal --test loom --release
//! ```
#![cfg(loom)]

use loom::sync::Arc;
use loom::thread;
use rustamp_metal::irq::IrqTable;
use rustamp_metal::sync::{Mutex, RawSpinLock};

#[test]
fn spin_mutex_excludes() {
    loom::model(|| {
        let m = Arc::new(Mutex::from_raw(RawSpinLock::new(), (0u32, 0u32)));
        let hs: Vec<_> = (0..2)
            .map(|_| {
                let m = m.clone();
                thread::spawn(move || {
                    let mut g = m.lock();
                    // Inside the lock, nobody else sees the half-updated pair.
                    g.0 += 1;
                    thread::yield_now();
                    g.1 += 1;
                    assert_eq!(g.0, g.1);
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(*m.lock(), (2, 2));
        let g = m.lock();
        assert!(m.try_lock().is_none());
        drop(g);
        assert!(m.try_lock().is_some());
    });
}

// Each handler checks it is called with its own argument.
fn handler_a(_: u32, arg: *mut ()) {
    assert_eq!(arg.addr(), 0xa, "handler A called with another argument");
}

fn handler_b(_: u32, arg: *mut ()) {
    assert_eq!(arg.addr(), 0xb, "handler B called with another argument");
}

fn arg(v: usize) -> *mut () {
    core::ptr::without_provenance_mut(v)
}

#[test]
fn irq_register_racing_dispatch() {
    loom::model(|| {
        let t = Arc::new(IrqTable::<1>::new());
        t.register(0, handler_a, arg(0xa)).unwrap();
        let t2 = t.clone();
        let w = thread::spawn(move || t2.register(0, handler_b, arg(0xb)).unwrap());
        t.dispatch(0);
        w.join().unwrap();
        // Once registration returned, the new handler is in place.
        assert!(t.dispatch(0));
    });
}

#[test]
fn irq_unregister_racing_dispatch() {
    loom::model(|| {
        let t = Arc::new(IrqTable::<1>::new());
        t.register(0, handler_a, arg(0xa)).unwrap();
        let t2 = t.clone();
        let w = thread::spawn(move || t2.unregister(0));
        t.dispatch(0);
        w.join().unwrap();
        assert!(!t.dispatch(0));
    });
}
