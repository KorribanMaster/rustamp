// SPDX-License-Identifier: BSD-3-Clause
//! Hardware and OS abstraction layer for OpenAMP, a Rust port of
//! [libmetal](https://github.com/OpenAMP/libmetal).
//!
//! The crate is `no_std` and provides the minimum surface the other
//! `openamp-*` crates need:
//!
//! * [`IoRegion`]: bounds-checked volatile and atomic access to shared memory
//!   or MMIO, with physical/device address translation. It is the **only**
//!   place in the workspace that turns addresses into raw pointers.
//! * [`cache::Cache`]: data-cache maintenance for non-coherent shared memory.
//! * [`notify::Notifier`], [`time::Delay`], [`irq::IrqController`]: platform
//!   services implemented per board.
//! * [`sync::Mutex`]: a lock with a pluggable [`sync::RawLock`].
//! * [`shmem::ShmemPool`]: a bump allocator over a shared region.
//!
//! * [`log`]: a pluggable diagnostics handler (`metal_log`).
//!
//! Backends: bare-metal (everything here) and, with the `linux` feature, Linux
//! userspace ([`linux`]: UIO devices and shared-memory files).
//!
//! Byte order: OpenAMP shared structures are little-endian on the wire. All
//! targets OpenAMP supports today are little-endian; the higher crates still
//! call `to_le`/`from_le` so a big-endian port only has to be tested, not
//! rewritten.
#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

pub mod cache;
mod error;
pub mod io;
pub mod irq;
#[macro_use]
pub mod log;
pub mod notify;
pub mod shmem;
pub mod sync;
pub mod time;

#[cfg(feature = "linux")]
pub mod linux;

pub use error::{Error, Result};
pub use io::IoRegion;

/// Rounds `value` up to the next multiple of `align` (a power of two).
#[inline]
pub const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// Rounds `value` down to a multiple of `align` (a power of two).
#[inline]
pub const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}
