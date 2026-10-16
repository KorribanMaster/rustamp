// SPDX-License-Identifier: BSD-3-Clause
//! Virtio for OpenAMP: split virtqueues in both roles, and the transport
//! interface rpmsg runs over. A Rust port of `open-amp/lib/virtio` and
//! `lib/virtio_mmio`.
//!
//! * [`ring::VringLayout`]: the split-ring memory layout, identical to the
//!   legacy `vring` used by Linux remoteproc (alignment from the resource
//!   table).
//! * [`DriverQueue`]: the side that owns buffers and posts them (Linux host,
//!   or an OpenAMP host).
//! * [`DeviceQueue`]: the side that consumes posted buffers and returns them
//!   (remote firmware).
//! * [`Transport`]: status, feature negotiation, config space, queue
//!   discovery and notification. Implemented by
//!   `openamp-remoteproc` (resource-table vdevs) and [`mmio::MmioTransport`].
//!
//! Everything read from shared memory is treated as untrusted: ring indices,
//! descriptor heads and chain links are validated, and violations surface as
//! [`Error::InvalidDescriptor`] rather than panics.
#![no_std]

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
