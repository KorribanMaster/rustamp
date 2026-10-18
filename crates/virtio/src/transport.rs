// SPDX-License-Identifier: BSD-3-Clause
//! The virtio transport interface (`struct virtio_dispatch`).

use crate::{QueueConfig, Result};

/// Which end of the virtio device this side is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Owns and posts buffers (rpmsg host).
    Driver,
    /// Consumes posted buffers (rpmsg remote).
    Device,
}

/// Access to a virtio device's control plane.
///
/// Methods take `&self`: transports are thin views over shared memory or
/// registers and keep no mutable state of their own.
pub trait Transport {
    /// Our role.
    fn role(&self) -> Role;

    /// Virtio device id (e.g. [`VIRTIO_ID_RPMSG`](crate::VIRTIO_ID_RPMSG)).
    fn device_id(&self) -> u32;

    /// Device status byte.
    fn status(&self) -> u8;

    /// Writes the device status byte (driver: progress; device: reset or
    /// `NEEDS_RESET`).
    fn set_status(&self, status: u8);

    /// Features offered by the device.
    fn device_features(&self) -> u64;

    /// Features accepted by the driver.
    fn driver_features(&self) -> u64;

    /// Accepts `features` (driver role).
    fn set_driver_features(&self, features: u64) -> Result<()>;

    /// Negotiated features: offered and accepted.
    fn features(&self) -> u64 {
        self.device_features() & self.driver_features()
    }

    /// Reads device configuration space at `offset`.
    fn read_config(&self, offset: usize, buf: &mut [u8]) -> Result<()>;

    /// Writes device configuration space at `offset`.
    fn write_config(&self, offset: usize, buf: &[u8]) -> Result<()>;

    /// Number of virtqueues.
    fn queue_count(&self) -> usize;

    /// Location and geometry of queue `index`.
    ///
    /// For transports where the driver chooses ring memory (MMIO), call
    /// [`setup_queue`](Self::setup_queue) first.
    fn queue(&self, index: usize) -> Result<QueueConfig>;

    /// Tells the device where the driver placed queue `index` (driver role,
    /// transports where ring memory is chosen by the driver).
    fn setup_queue(&self, index: usize, cfg: &QueueConfig) -> Result<()> {
        let _ = (index, cfg);
        Err(crate::Error::Unsupported)
    }

    /// Notifies the peer about queue `index`.
    fn notify(&self, index: usize);
}

impl<T: Transport + ?Sized> Transport for &T {
    fn role(&self) -> Role {
        (**self).role()
    }
    fn device_id(&self) -> u32 {
        (**self).device_id()
    }
    fn status(&self) -> u8 {
        (**self).status()
    }
    fn set_status(&self, status: u8) {
        (**self).set_status(status)
    }
    fn device_features(&self) -> u64 {
        (**self).device_features()
    }
    fn driver_features(&self) -> u64 {
        (**self).driver_features()
    }
    fn set_driver_features(&self, features: u64) -> Result<()> {
        (**self).set_driver_features(features)
    }
    fn features(&self) -> u64 {
        (**self).features()
    }
    fn read_config(&self, offset: usize, buf: &mut [u8]) -> Result<()> {
        (**self).read_config(offset, buf)
    }
    fn write_config(&self, offset: usize, buf: &[u8]) -> Result<()> {
        (**self).write_config(offset, buf)
    }
    fn queue_count(&self) -> usize {
        (**self).queue_count()
    }
    fn queue(&self, index: usize) -> Result<QueueConfig> {
        (**self).queue(index)
    }
    fn setup_queue(&self, index: usize, cfg: &QueueConfig) -> Result<()> {
        (**self).setup_queue(index, cfg)
    }
    fn notify(&self, index: usize) {
        (**self).notify(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::VringLayout;
    use core::cell::Cell;
    use rustamp_metal::IoRegion;

    /// A transport implementing only the required methods.
    struct Minimal {
        status: Cell<u8>,
        driver: Cell<u64>,
        notified: Cell<Option<usize>>,
    }

    impl Transport for Minimal {
        fn role(&self) -> Role {
            Role::Driver
        }
        fn device_id(&self) -> u32 {
            7
        }
        fn status(&self) -> u8 {
            self.status.get()
        }
        fn set_status(&self, status: u8) {
            self.status.set(status)
        }
        fn device_features(&self) -> u64 {
            0b1011
        }
        fn driver_features(&self) -> u64 {
            self.driver.get()
        }
        fn set_driver_features(&self, features: u64) -> Result<()> {
            self.driver.set(features);
            Ok(())
        }
        fn read_config(&self, offset: usize, buf: &mut [u8]) -> Result<()> {
            buf.fill(offset as u8);
            Ok(())
        }
        fn write_config(&self, _: usize, _: &[u8]) -> Result<()> {
            Err(crate::Error::Unsupported)
        }
        fn queue_count(&self) -> usize {
            1
        }
        fn queue(&self, index: usize) -> Result<QueueConfig> {
            if index != 0 {
                return Err(crate::Error::NoSuchQueue);
            }
            Ok(QueueConfig {
                io: IoRegion::alloc(0x1000, 0),
                offset: 0,
                layout: VringLayout::new(4, 16)?,
                notify_id: 0,
            })
        }
        fn notify(&self, index: usize) {
            self.notified.set(Some(index))
        }
    }

    /// The provided methods, and every method through `&T`.
    #[test]
    fn defaults_and_forwarding() {
        let t = Minimal {
            status: Cell::new(0),
            driver: Cell::new(0),
            notified: Cell::new(None),
        };
        let r: &dyn Transport = &t;
        let rr = &r;
        assert_eq!(rr.role(), Role::Driver);
        assert_eq!(rr.device_id(), 7);
        rr.set_status(3);
        assert_eq!(rr.status(), 3);
        assert_eq!(rr.device_features(), 0b1011);
        rr.set_driver_features(0b0110).unwrap();
        assert_eq!(rr.driver_features(), 0b0110);
        // Default: offered & accepted.
        assert_eq!(rr.features(), 0b0010);
        let mut b = [0u8; 2];
        rr.read_config(9, &mut b).unwrap();
        assert_eq!(b, [9, 9]);
        assert_eq!(rr.write_config(0, &b), Err(crate::Error::Unsupported));
        assert_eq!(rr.queue_count(), 1);
        let q = rr.queue(0).unwrap();
        assert_eq!(rr.queue(1).unwrap_err(), crate::Error::NoSuchQueue);
        // Default: the device chooses ring memory.
        assert_eq!(rr.setup_queue(0, &q), Err(crate::Error::Unsupported));
        rr.notify(0);
        assert_eq!(t.notified.get(), Some(0));
    }
}
