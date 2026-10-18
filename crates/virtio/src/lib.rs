// SPDX-License-Identifier: BSD-3-Clause
//! Virtio split virtqueues and the transport interface for OpenAMP: a
//! `no_std` port of open-amp's `lib/virtio` and `lib/virtio_mmio`.
//!
//! Virtio is OpenAMP's media-access layer: rpmsg messages travel in buffers
//! passed through two vrings in shared memory.
//!
//! Part of [RustAmp](https://github.com/KorribanMaster/openamp-rs):
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
//! * [`ring::VringLayout`]: the split-ring layout, identical to the legacy
//!   vring Linux remoteproc uses.
//! * [`DriverQueue`]: owns buffers and posts them (the host).
//! * [`DeviceQueue`]: takes posted buffers and returns them (the remote).
//! * [`Transport`]: status, feature negotiation, config space, queue
//!   discovery and notification. Implemented by `rustamp-remoteproc`
//!   (resource-table vdevs) and [`mmio::MmioTransport`].
//!
//! Everything the peer writes is untrusted. Bad indices, descriptor heads or
//! chains become [`Error::InvalidDescriptor`], never a panic or an
//! out-of-bounds access.
//!
//! ```
//! use rustamp_metal::IoRegion;
//! use rustamp_virtio::ring::VringLayout;
//! use rustamp_virtio::{Buffer, DeviceQueue, DriverQueue, QueueConfig};
//!
//! let layout = VringLayout::new(8, 64)?;
//! let cfg = QueueConfig {
//!     io: IoRegion::alloc(layout.size(), 0x5500_0000),
//!     offset: 0,
//!     layout,
//!     notify_id: 0,
//! };
//! let mut driver = DriverQueue::new(&cfg, false)?;
//! let mut device = DeviceQueue::new(&cfg, false)?;
//!
//! driver.add(&[Buffer { addr: 0x5540_0000, len: 512, device_writable: true }])?;
//! let buf = device.pop_avail()?.expect("posted");
//! device.push_used(buf, 42)?; // "wrote 42 bytes"
//! assert_eq!(driver.pop_used()?.map(|u| u.len), Some(42));
//! # Ok::<(), rustamp_virtio::Error>(())
//! ```
//!
//! # Features
//!
//! | Feature | Adds |
//! |---|---|
//! | `mmio` (default) | [`mmio::MmioTransport`], the virtio-mmio driver |
//! | `log`, `defmt` | `defmt::Format` on [`Error`] |
#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(test)]
extern crate std;

mod error;
#[cfg(feature = "mmio")]
pub mod mmio;
mod queue;
pub mod ring;
mod transport;

pub use error::{Error, Result};
pub use queue::{AvailBuf, Buffer, Desc, DeviceQueue, DriverQueue, QueueConfig, Used};
pub use transport::{Role, Transport};

/// Virtio device id of an rpmsg device (`VIRTIO_ID_RPMSG`).
pub const VIRTIO_ID_RPMSG: u32 = 7;

/// Device status bits (`VIRTIO_CONFIG_STATUS_*`).
pub mod status {
    /// The driver has noticed the device.
    pub const ACKNOWLEDGE: u8 = 0x01;
    /// The driver knows how to drive the device.
    pub const DRIVER: u8 = 0x02;
    /// The driver is set up and ready.
    pub const DRIVER_OK: u8 = 0x04;
    /// Feature negotiation is complete (virtio 1.0 and later).
    pub const FEATURES_OK: u8 = 0x08;
    /// The device has hit an error it cannot recover from without reset.
    pub const NEEDS_RESET: u8 = 0x40;
    /// The driver gave up on the device.
    pub const FAILED: u8 = 0x80;
}

/// Device-independent feature bits.
pub mod features {
    /// Indirect descriptors (`VIRTIO_RING_F_INDIRECT_DESC`); not used by
    /// rpmsg and not supported by this crate.
    pub const RING_INDIRECT_DESC: u32 = 28;
    /// `used_event`/`avail_event` notification suppression
    /// (`VIRTIO_RING_F_EVENT_IDX`).
    pub const RING_EVENT_IDX: u32 = 29;
    /// Virtio 1.0 compliance (`VIRTIO_F_VERSION_1`).
    pub const VERSION_1: u32 = 32;

    /// Whether `bit` is set in `features`.
    #[inline]
    pub const fn has(features: u64, bit: u32) -> bool {
        features & (1 << bit) != 0
    }
}
