// SPDX-License-Identifier: BSD-3-Clause
//! Remote processor support, a Rust port of `open-amp/lib/remoteproc`.
//!
//! Two audiences:
//!
//! * **Firmware** (the remote): declare the resource table with
//!   [`resource_table!`], then turn its vdev into a virtio [`Transport`]
//!   with [`RprocVirtio::new_device`] and hand that to `openamp-rpmsg`.
//! * **Hosts** written with this crate: [`Remoteproc`] loads an ELF image
//!   through [`RemoteprocOps`], parses the resource table, allocates vrings
//!   and provides the driver-side [`RprocVirtio`]. (On Linux hosts the
//!   kernel's remoteproc framework does this instead.)
//!
//! [`Transport`]: openamp_virtio::Transport
#![no_std]

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
