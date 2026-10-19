// SPDX-License-Identifier: BSD-3-Clause
//! The remoteproc trace buffer: Linux shows it in
//! `/sys/kernel/debug/remoteproc/remoteproc0/trace0`.
//!
//! A NUL-terminated log that stops when full. `memory.x` places it at
//! `board::TRACE_DA`, the address the resource table announces.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use zynqmp_r5_echo::board::TRACE_LEN;

#[repr(transparent)]
struct Buf(UnsafeCell<[u8; TRACE_LEN as usize]>);

// SAFETY: written only by `put`, which is only called from thread context
// (the IRQ handler does not log), and read only by Linux.
unsafe impl Sync for Buf {}

#[unsafe(link_section = ".rsc_trace")]
#[used]
static TRACE: Buf = Buf(UnsafeCell::new([0; TRACE_LEN as usize]));
static POS: AtomicUsize = AtomicUsize::new(0);

/// Appends `b`, keeping a terminating NUL.
pub fn put(b: u8) {
    let i = POS.load(Ordering::Relaxed);
    if i + 1 >= TRACE_LEN as usize {
        return;
    }
    let p = TRACE.0.get().cast::<u8>();
    // SAFETY: in bounds (checked above); see the `Sync` impl.
    unsafe {
        p.add(i).write_volatile(b);
        p.add(i + 1).write_volatile(0);
    }
    POS.store(i + 1, Ordering::Relaxed);
}
