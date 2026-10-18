// SPDX-License-Identifier: BSD-3-Clause
//! Remote processor lifecycle, resource tables and ELF loading for OpenAMP:
//! a `no_std` port of open-amp's `lib/remoteproc`, compatible with Linux
//! remoteproc.
//!
//! Part of [openamp-rs](https://github.com/KorribanMaster/openamp-rs):
//!
//! ```text
//! openamp-proxy ──► openamp-rpmsg ──────┐
//!                                       ├──► openamp-virtio ──► openamp-metal
//! openamp-remoteproc ───────────────────┘
//!   (supplies the virtio Transport that rpmsg runs over)
//! ```
//!
//! In OpenAMP terms, the *host* here is the main controller (often Linux),
//! and the *remote* is the core it loads, starts and talks to.
//!
//! # Resource table
//!
//! The firmware describes what it needs in a *resource table*, an ELF
//! section named `.resource_table`: memory carveouts, a trace buffer, and
//! virtio devices (vdevs) with their vrings. The host reads it when loading
//! the image and writes back what it set up (vring addresses, notify ids,
//! negotiated features, device status).
//!
//! # Firmware (the remote)
//!
//! Declare the table with [`resource_table!`], then turn its vdev into the
//! virtio [`Transport`](openamp_virtio::Transport) that rpmsg runs over:
//!
//! ```no_run
//! use openamp_metal::IoRegion;
//! use openamp_remoteproc::rsc::{FwRscVdev, FwRscVdevVring, ResourceTable};
//! use openamp_remoteproc::{RprocVirtio, resource_table};
//! use openamp_rpmsg::{Options, RpmsgVirtio, VIRTIO_RPMSG_F_NS};
//! use openamp_virtio::VIRTIO_ID_RPMSG;
//!
//! resource_table! {
//!     static RESOURCE_TABLE: RscTable = {
//!         vdev: FwRscVdev<2> = FwRscVdev::new(VIRTIO_ID_RPMSG, 0, 1 << VIRTIO_RPMSG_F_NS, [
//!             FwRscVdevVring::new(0x5500_0000, 0x1000, 256, 0),
//!             FwRscVdevVring::new(0x5500_8000, 0x1000, 256, 1),
//!         ]),
//!     };
//! }
//!
//! # fn kick_host(_: u32) {}
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // SAFETY: memory reserved for OpenAMP in the host's device tree.
//! let shm = unsafe { IoRegion::new(0x5500_0000 as *mut u8, 0x5500_0000, 0x50_0000) };
//! let table = ResourceTable::new(RESOURCE_TABLE.io_region(), 0, RESOURCE_TABLE.size())?;
//! let transport = RprocVirtio::new_device(table.find_vdev(0)?, shm.clone(), kick_host);
//! // Waits until the host has set up the vdev.
//! let rpmsg: RpmsgVirtio<_> = RpmsgVirtio::new_remote(transport, shm, Options::default())?;
//! # Ok(()) }
//! ```
//!
//! # Hosts
//!
//! On Linux, the kernel's remoteproc framework is the host. Hosts written
//! with this crate use [`Remoteproc`]: implement [`RemoteprocOps`] (map
//! memory, start, stop), then `config` → `load` an ELF image (streamed
//! through [`elf::ImageStore`]) → [`Remoteproc::create_vdev`] for the
//! driver-side transport → `start`. The ELF loader and the table parser
//! treat the image and table as untrusted input.
//!
//! # Features
//!
//! None are on by default.
//!
//! | Feature | Adds |
//! |---|---|
//! | `std` | [`elf::ImageStore`] for `std::fs::File` |
//! | `log`, `defmt` | Diagnostics (skipped resources); `defmt::Format` on [`Error`] |
#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(any(feature = "std", test))]
extern crate std;

#[macro_use]
mod fmt;

pub mod elf;
mod error;
mod rproc;
pub mod rsc;
mod virtio;

pub use error::{Error, Result};
pub use rproc::{LoadInfo, Remoteproc, RemoteprocOps, State};
pub use virtio::RprocVirtio;

#[doc(hidden)]
#[macro_export]
macro_rules! __count {
    () => { 0usize };
    ($head:tt $($tail:tt)*) => { 1usize + $crate::__count!($($tail)*) };
}

/// Declares a resource table in the `.resource_table` section.
///
/// Generates a `#[repr(C)]` struct holding a [`rsc::RscTableHeader`] plus
/// the listed entries, with the header's offsets computed by the compiler,
/// and a `static` [`rsc::SharedTable`] holding it.
///
/// ```
/// use openamp_remoteproc::resource_table;
/// use openamp_remoteproc::rsc::{FwRscCarveout, FwRscVdev, FwRscVdevVring, FW_RSC_ADDR_ANY};
///
/// resource_table! {
///     pub static RESOURCE_TABLE: MyTable = {
///         carveout: FwRscCarveout = FwRscCarveout::new("fw", 0x1ffe_0000, 0x1ffe_0000, 0x2_0000, 0),
///         vdev: FwRscVdev<2> = FwRscVdev::new(7, 0, 1, [
///             FwRscVdevVring::new(0x5500_0000, 0x1000, 256, 0),
///             FwRscVdevVring::new(0x5500_8000, 0x1000, 256, 1),
///         ]),
///     };
/// }
/// assert_eq!(core::mem::size_of::<MyTable>(), 24 + 56 + 68);
/// ```
#[macro_export]
macro_rules! resource_table {
    (
        $(#[$attr:meta])*
        $vis:vis static $name:ident: $ty:ident = {
            $($field:ident: $fty:ty = $init:expr),+ $(,)?
        };
    ) => {
        #[repr(C)]
        #[allow(missing_docs)]
        $vis struct $ty {
            pub header: $crate::rsc::RscTableHeader<{ $crate::__count!($($field)+) }>,
            $(pub $field: $fty,)+
        }

        $(#[$attr])*
        #[used]
        #[unsafe(link_section = ".resource_table")]
        $vis static $name: $crate::rsc::SharedTable<$ty> =
            $crate::rsc::SharedTable::new($ty {
                header: $crate::rsc::RscTableHeader::new([
                    $(::core::mem::offset_of!($ty, $field) as u32),+
                ]),
                $($field: $init,)+
            });
    };
}
