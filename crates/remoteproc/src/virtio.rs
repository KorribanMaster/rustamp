// SPDX-License-Identifier: BSD-3-Clause
//! A virtio transport backed by a resource-table vdev
//! (`remoteproc_virtio.c`).

use openamp_metal::IoRegion;
use openamp_metal::cache::{Cache, NoCache};
use openamp_metal::notify::Notifier;
use openamp_virtio::ring::VringLayout;
use openamp_virtio::{QueueConfig, Role, Transport};

use crate::rsc::{FW_RSC_ADDR_ANY, VdevRsc};

/// A vdev from a resource table, as a virtio [`Transport`].
///
/// Status and feature negotiation go through the table (the host writes
/// `status` and `gfeatures`), vrings are located through their `da`, and
/// notifications use the vring notify ids.
///
/// `shm` must cover the vrings and message buffers, with `shm.phys()` in
/// the address space the table's `da` values use (the remote's view; on
/// most SoCs the same as the physical address).
pub struct RprocVirtio<N: Notifier> {
    role: Role,
    vdev: VdevRsc,
    shm: IoRegion,
    notifier: N,
    cache: &'static dyn Cache,
}

impl<N: Notifier> RprocVirtio<N> {
    /// The firmware side (virtio device, rpmsg remote).
    pub fn new_device(vdev: VdevRsc, shm: IoRegion, notifier: N) -> Self {
        RprocVirtio {
            role: Role::Device,
            vdev,
            shm,
            notifier,
            cache: &NoCache,
        }
    }

    /// The host side (virtio driver, rpmsg host). Vring addresses must
    /// already be set in the table; [`Remoteproc::create_vdev`] does this.
    ///
    /// [`Remoteproc::create_vdev`]: crate::Remoteproc::create_vdev
    pub fn new_driver(vdev: VdevRsc, shm: IoRegion, notifier: N) -> Self {
        RprocVirtio {
            role: Role::Driver,
            vdev,
            shm,
            notifier,
            cache: &NoCache,
        }
    }

    /// Uses `cache` to keep a cacheable resource table coherent
    /// (`WITH_DCACHE_RSC_TABLE`).
    pub fn with_cache(mut self, cache: &'static dyn Cache) -> Self {
        self.cache = cache;
        self
    }

    /// The vdev entry.
    pub fn vdev(&self) -> &VdevRsc {
        &self.vdev
    }

    /// The notifier.
    pub fn notifier(&self) -> &N {
        &self.notifier
    }

    fn rsc_ptr(&self) -> (*const u8, usize) {
        let p = self
            .vdev
            .io()
            .offset_to_virt(self.vdev.offset())
            .unwrap_or(core::ptr::null_mut());
        (p, self.vdev.config_offset() - self.vdev.offset())
    }

    fn invalidate(&self) {
        let (p, len) = self.rsc_ptr();
        self.cache.invalidate(p, len);
    }

    fn flush(&self) {
        let (p, len) = self.rsc_ptr();
        self.cache.flush(p, len);
    }

    /// Handles a notification for `notifyid` (`rproc_virtio_notified`):
    /// returns the index of the vring it refers to, if any.
    pub fn vring_for_notifyid(&self, notifyid: u32) -> Option<usize> {
        (0..self.vdev.num_vrings())
            .find(|&i| self.vdev.vring(i).map(|v| v.notifyid) == Ok(notifyid))
    }
}

impl<N: Notifier> Transport for RprocVirtio<N> {
    fn role(&self) -> Role {
        self.role
    }

    fn device_id(&self) -> u32 {
        self.vdev.id()
    }

    fn status(&self) -> u8 {
        self.invalidate();
        self.vdev.status()
    }

    fn set_status(&self, status: u8) {
        let _ = self.vdev.set_status(status);
        self.flush();
        self.notifier.notify(self.vdev.notifyid());
    }

    fn device_features(&self) -> u64 {
        self.invalidate();
        self.vdev.dfeatures() as u64
    }

    fn driver_features(&self) -> u64 {
        self.invalidate();
        self.vdev.gfeatures() as u64
    }

    fn set_driver_features(&self, features: u64) -> openamp_virtio::Result<()> {
        if self.role != Role::Driver {
            return Err(openamp_virtio::Error::Unsupported);
        }
        self.vdev.set_gfeatures(features as u32)?;
        self.flush();
        Ok(())
    }

    fn read_config(&self, offset: usize, buf: &mut [u8]) -> openamp_virtio::Result<()> {
        if offset.checked_add(buf.len()).is_none_or(|e| e > self.vdev.config_len() as usize) {
            return Err(openamp_virtio::Error::Metal(openamp_metal::Error::OutOfBounds));
        }
        Ok(self
            .vdev
            .io()
            .read_bytes(self.vdev.config_offset() + offset, buf)?)
    }

    fn write_config(&self, offset: usize, buf: &[u8]) -> openamp_virtio::Result<()> {
        if offset.checked_add(buf.len()).is_none_or(|e| e > self.vdev.config_len() as usize) {
            return Err(openamp_virtio::Error::Metal(openamp_metal::Error::OutOfBounds));
        }
        self.vdev
            .io()
            .write_bytes(self.vdev.config_offset() + offset, buf)?;
        self.flush();
        Ok(())
    }

    fn queue_count(&self) -> usize {
        self.vdev.num_vrings()
    }

    fn queue(&self, index: usize) -> openamp_virtio::Result<QueueConfig> {
        self.invalidate();
        let v = self.vdev.vring(index)?;
        if v.da == FW_RSC_ADDR_ANY {
            return Err(openamp_virtio::Error::NotReady);
        }
        let num = u16::try_from(v.num).map_err(|_| openamp_virtio::Error::BadLayout)?;
        let layout = VringLayout::new(num, v.align as usize)?;
        let offset = self.shm.phys_range_to_offset(v.da as u64, layout.size())?;
        Ok(QueueConfig {
            io: self.shm.clone(),
            offset,
            layout,
            notify_id: v.notifyid,
        })
    }

    fn notify(&self, index: usize) {
        if let Ok(v) = self.vdev.vring(index) {
            self.notifier.notify(v.notifyid);
        }
    }
}
