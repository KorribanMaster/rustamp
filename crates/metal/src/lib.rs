// SPDX-License-Identifier: BSD-3-Clause
//! Hardware and OS abstraction for OpenAMP: a `no_std` port of
//! [libmetal](https://github.com/OpenAMP/libmetal).
//!
//! Part of [RustAmp](https://github.com/KorribanMaster/rustamp):
//!
//! ```text
//! rustamp-proxy ──► rustamp-rpmsg ──────┐
//!                                       ├──► rustamp-virtio ──► rustamp-metal
//! rustamp-remoteproc ───────────────────┘
//!   (supplies the virtio Transport that rpmsg runs over)
//! ```
//!
//! In OpenAMP terms, the *host* here is the main controller (often Linux),
//! and the *remote* is the core it loads, starts and talks to.
//!
//! # What it provides
//!
//! * [`IoRegion`]: bounds-checked volatile, atomic and block access to shared
//!   memory or MMIO, with physical/device address translation. It is the only
//!   place in RustAmp that turns addresses into raw pointers.
//! * [`cache`]: data-cache maintenance for non-coherent shared memory.
//! * [`notify::Notifier`], [`time::Delay`], [`irq`]: the per-board hooks
//!   (inter-processor interrupt, waiting, interrupt dispatch).
//! * [`sync::Mutex`] over a pluggable [`sync::RawLock`]: a spin lock, or
//!   `CsLock` (masks interrupts) for rpmsg used from interrupt handlers.
//! * [`shmem::ShmemPool`]: a bump allocator for vrings and message buffers.
//! * [`linux`]: Linux userspace backend (UIO devices, shared-memory files).
//!
//! ```
//! use rustamp_metal::IoRegion;
//!
//! // On firmware: `unsafe { IoRegion::new(virt, phys, len) }` over memory
//! // reserved for OpenAMP. On a host, for tests:
//! let shm = IoRegion::alloc(0x1000, 0x5500_0000);
//! shm.write_u32(0x10, 0xdead_beef)?;
//! assert_eq!(shm.read_u32(0x10)?, 0xdead_beef);
//! assert_eq!(shm.offset_to_phys(0x10), Some(0x5500_0010));
//! assert!(shm.read_u32(0x1000).is_err()); // out of bounds is an error
//! # Ok::<(), rustamp_metal::Error>(())
//! ```
//!
//! # Features
//!
//! None are on by default.
//!
//! | Feature | Adds |
//! |---|---|
//! | `alloc` | [`IoRegion::alloc`], for host-side tests |
//! | `std` | `alloc`, `std` error impls, a sleeping delay |
//! | `linux` | `std` and the [`linux`] backend |
//! | `critical-section` | `sync::CsLock` |
//! | `cortex-m7`, `armv7a` | Cache maintenance for that core |
//! | `log`, `defmt` | `defmt::Format` on [`Error`]; forwarded to by the higher crates |
//!
//! OpenAMP shared structures are little-endian; all data is converted
//! explicitly, so a big-endian port only needs testing.
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
