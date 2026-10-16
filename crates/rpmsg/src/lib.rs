// SPDX-License-Identifier: BSD-3-Clause
//! RPMsg over virtio, a Rust port of `open-amp/lib/rpmsg`.
//!
//! Wire-compatible with the Linux kernel's `virtio_rpmsg_bus` and with C
//! OpenAMP in both roles:
//!
//! * **Remote** (virtio device role): firmware talking to Linux or to an
//!   OpenAMP host. Created with [`RpmsgVirtio::new_remote`].
//! * **Host** (virtio driver role): owns the message buffers. Created with
//!   [`RpmsgVirtio::new_host`].
//!
//! The API is polled first: [`RpmsgVirtio::try_recv`] returns one message (or
//! name-service event) at a time, and the message buffer goes back to the
//! peer when the returned [`RxMessage`] is dropped, so holding a message is
//! zero-copy. Sending copies into a ring buffer ([`RpmsgVirtio::send`]) or
//! fills one in place ([`RpmsgVirtio::get_tx_buffer`]). With the `async`
//! feature, `recv().await`/`send_async().await` sit on top, woken by
//! [`RpmsgVirtio::on_notify`].
//!
//! ```no_run
//! # use openamp_rpmsg::*;
//! # fn demo<T: openamp_virtio::Transport>(rpmsg: RpmsgVirtio<T>) -> Result<()> {
//! let ept = rpmsg.create_endpoint("rpmsg-client-sample", ADDR_ANY, ADDR_ANY)?;
//! loop {
//!     if let Some(Received::Message(msg)) = rpmsg.try_recv()? {
//!         if msg.endpoint() == ept {
//!             rpmsg.send_to(ept, msg.src(), msg.data())?; // echo
//!         }
//!     }
//! }
//! # }
//! ```
#![no_std]

#[cfg(test)]
extern crate std;

mod device;
mod error;
mod name;
pub mod wire;

pub use device::{
    Endpoint, HostConfig, NsEvent, NsKind, Options, Received, RpmsgVirtio, RxMessage, TxBuffer,
};
pub use error::{Error, Result};
pub use name::Name;

/// "Any address": let the stack pick, or "destination not yet known".
pub const ADDR_ANY: u32 = 0xFFFF_FFFF;
/// Address of the name-service endpoint.
pub const NS_EPT_ADDR: u32 = 0x35;
/// Addresses below this are reserved for fixed services.
pub const RESERVED_ADDRESSES: u32 = 1024;
/// Number of dynamically allocatable addresses (`RPMSG_ADDR_BMP_SIZE`).
pub const ADDR_BMP_SIZE: u32 = 128;
/// Maximum endpoint name length including the terminating NUL.
pub const NAME_SIZE: usize = 32;
/// Default buffer size, as hard-coded in the Linux kernel.
pub const DEFAULT_BUFFER_SIZE: u32 = 512;
/// Virtio feature bit: the device supports name-service announcements.
pub const VIRTIO_RPMSG_F_NS: u32 = 0;
