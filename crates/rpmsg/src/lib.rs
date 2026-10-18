// SPDX-License-Identifier: BSD-3-Clause
//! RPMsg, OpenAMP's inter-processor messaging, over virtio: a `no_std`
//! port of open-amp's `lib/rpmsg`, wire-compatible with the Linux kernel's
//! `virtio_rpmsg_bus` and with C OpenAMP.
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
//! # Model
//!
//! Messages go between *endpoints*, each with a 32-bit address. A message
//! carries a 16-byte header (source, destination, length) and its payload in
//! one shared buffer. The *name service* announces endpoints by name, so
//! Linux binds a driver to them (`rpmsg-tty` gives `/dev/ttyRPMSG*`).
//!
//! * Remote (virtio device role): [`RpmsgVirtio::new_remote`], usually over
//!   `rustamp_remoteproc::RprocVirtio`.
//! * Host (virtio driver role, owns the buffers): [`RpmsgVirtio::new_host`].
//!
//! The API is polled first. [`RpmsgVirtio::try_recv`] returns one message or
//! name-service event at a time; an [`RxMessage`] holds its buffer until
//! dropped, so receiving is zero-copy. Sending copies into a buffer
//! ([`RpmsgVirtio::send`]) or fills one in place
//! ([`RpmsgVirtio::get_tx_buffer`]).
//!
//! ```no_run
//! # use rustamp_rpmsg::*;
//! # fn demo<T: rustamp_virtio::Transport>(rpmsg: RpmsgVirtio<T>) -> Result<()> {
//! // Announced to the host as "rpmsg-tty".
//! let ept = rpmsg.create_endpoint("rpmsg-tty", ADDR_ANY, ADDR_ANY)?;
//! loop {
//!     if let Some(Received::Message(msg)) = rpmsg.try_recv()? {
//!         rpmsg.send_to(ept, msg.src(), msg.data())?; // echo
//!     } // the buffer goes back to the host when `msg` drops
//! }
//! # }
//! ```
//!
//! The lock type `L` of [`RpmsgVirtio`] defaults to a spin lock, which must
//! not be taken from an interrupt handler. Call [`RpmsgVirtio::on_notify`]
//! from the interrupt and do the work in thread context. If you do need rpmsg
//! in an interrupt handler, use `rustamp_metal::sync::CsLock`.
//!
//! # Features
//!
//! None are on by default.
//!
//! | Feature | Adds |
//! |---|---|
//! | `async` | `recv().await`, `send_async().await`, woken by [`RpmsgVirtio::on_notify`] |
//! | `log` | Diagnostics (dropped malformed messages) through `log` |
//! | `defmt` | The same through `defmt`, and `defmt::Format` on [`Error`] |
#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(test)]
extern crate std;

#[macro_use]
mod fmt;

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
