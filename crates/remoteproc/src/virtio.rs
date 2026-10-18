// SPDX-License-Identifier: BSD-3-Clause
//! A virtio transport backed by a resource-table vdev
//! (`remoteproc_virtio.c`).

use rustamp_metal::IoRegion;
use rustamp_metal::cache::{Cache, NoCache};
use rustamp_metal::notify::Notifier;
use rustamp_virtio::ring::VringLayout;
use rustamp_virtio::{QueueConfig, Role, Transport};

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

    fn set_driver_features(&self, features: u64) -> rustamp_virtio::Result<()> {
        if self.role != Role::Driver {
            return Err(rustamp_virtio::Error::Unsupported);
        }
        self.vdev.set_gfeatures(features as u32)?;
        self.flush();
        Ok(())
    }

    fn read_config(&self, offset: usize, buf: &mut [u8]) -> rustamp_virtio::Result<()> {
        if offset
            .checked_add(buf.len())
            .is_none_or(|e| e > self.vdev.config_len() as usize)
        {
            return Err(rustamp_virtio::Error::Metal(
                rustamp_metal::Error::OutOfBounds,
            ));
        }
        Ok(self
            .vdev
            .io()
            .read_bytes(self.vdev.config_offset() + offset, buf)?)
    }

    fn write_config(&self, offset: usize, buf: &[u8]) -> rustamp_virtio::Result<()> {
        if offset
            .checked_add(buf.len())
            .is_none_or(|e| e > self.vdev.config_len() as usize)
        {
            return Err(rustamp_virtio::Error::Metal(
                rustamp_metal::Error::OutOfBounds,
            ));
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

    fn queue(&self, index: usize) -> rustamp_virtio::Result<QueueConfig> {
        self.invalidate();
        let v = self.vdev.vring(index)?;
        if v.da == FW_RSC_ADDR_ANY {
            return Err(rustamp_virtio::Error::NotReady);
        }
        let num = u16::try_from(v.num).map_err(|_| rustamp_virtio::Error::BadLayout)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsc::ResourceTable;
    use core::cell::RefCell;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::vec::Vec;

    const SHM_PHYS: u64 = 0x4000_0000;
    const VRING0_DA: u32 = 0x4000_1000;

    /// A table with one vdev: vring 0 placed, vring 1 still `ANY`, and an
    /// 8-byte config space.
    fn table() -> ResourceTable {
        let mut b = Vec::new();
        let w = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
        for v in [1, 1, 0, 0, 20] {
            w(&mut b, v); // ver, num, reserved, offset[0]
        }
        for v in [3, 7, 9, 0b101, 0, 8] {
            w(&mut b, v); // type, id, notifyid, dfeatures, gfeatures, config_len
        }
        b.extend_from_slice(&[0, 2, 0, 0]); // status, num_of_vrings, reserved
        for v in [VRING0_DA, 16, 4, 1, 0, FW_RSC_ADDR_ANY, 16, 4, 2, 0] {
            w(&mut b, v);
        }
        b.extend_from_slice(b"cfgspace");
        let io = IoRegion::alloc(b.len(), 0);
        io.write_bytes(0, &b).unwrap();
        ResourceTable::new(io, 0, b.len()).unwrap()
    }

    fn shm() -> IoRegion {
        IoRegion::alloc(0x4000, SHM_PHYS)
    }

    /// Counts cache operations on the resource table.
    struct Counting {
        flushes: AtomicUsize,
        invalidates: AtomicUsize,
    }

    impl Cache for Counting {
        fn flush(&self, _: *const u8, len: usize) {
            assert_eq!(len, 28 + 2 * 20, "the vdev entry and its vrings");
            self.flushes.fetch_add(1, Ordering::Relaxed);
        }
        fn invalidate(&self, _: *const u8, _: usize) {
            self.invalidates.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn driver_negotiates_through_the_table() {
        static CACHE: Counting = Counting {
            flushes: AtomicUsize::new(0),
            invalidates: AtomicUsize::new(0),
        };
        let kicks = RefCell::new(Vec::new());
        let t = table();
        let v = t.find_vdev(0).unwrap();
        let rv =
            RprocVirtio::new_driver(v, shm(), |id| kicks.borrow_mut().push(id)).with_cache(&CACHE);

        assert_eq!(rv.role(), Role::Driver);
        assert_eq!(rv.device_id(), 7);
        assert_eq!(rv.device_features(), 0b101);
        rv.set_driver_features(0b100).unwrap();
        assert_eq!(rv.driver_features(), 0b100);
        assert_eq!(rv.vdev().gfeatures(), 0b100);
        rv.set_status(4);
        assert_eq!(rv.status(), 4);
        // Status changes notify the vdev's own notify id.
        assert_eq!(*kicks.borrow(), [9]);
        assert!(CACHE.flushes.load(Ordering::Relaxed) >= 2);
        assert!(CACHE.invalidates.load(Ordering::Relaxed) >= 3);

        // Config space, bounds-checked against config_len.
        let mut c = [0u8; 8];
        rv.read_config(0, &mut c).unwrap();
        assert_eq!(&c, b"cfgspace");
        rv.write_config(3, b"XY").unwrap();
        rv.read_config(2, &mut c[..4]).unwrap();
        assert_eq!(&c[..4], b"gXYa");
        let oob = Err(rustamp_virtio::Error::Metal(
            rustamp_metal::Error::OutOfBounds,
        ));
        assert_eq!(rv.read_config(7, &mut [0; 2]), oob);
        assert_eq!(rv.write_config(usize::MAX, b"x"), oob);

        // Vrings: located through their da; one not placed yet.
        assert_eq!(rv.queue_count(), 2);
        let q = rv.queue(0).unwrap();
        assert_eq!((q.offset, q.notify_id, q.layout.num()), (0x1000, 1, 4));
        assert_eq!(rv.queue(1).unwrap_err(), rustamp_virtio::Error::NotReady);
        assert!(rv.queue(2).is_err());
        assert_eq!(rv.vring_for_notifyid(2), Some(1));
        assert_eq!(rv.vring_for_notifyid(5), None);
        rv.notify(1);
        rv.notify(9); // no such vring: nothing sent
        assert_eq!(*kicks.borrow(), [9, 2]);
        let _ = rv.notifier();
    }

    #[test]
    fn device_cannot_set_driver_features() {
        let t = table();
        let rv = RprocVirtio::new_device(t.find_vdev(0).unwrap(), shm(), |_: u32| {});
        assert_eq!(rv.role(), Role::Device);
        assert_eq!(
            rv.set_driver_features(1),
            Err(rustamp_virtio::Error::Unsupported)
        );
    }

    #[test]
    fn bad_vrings_are_rejected() {
        let t = table();
        let v = t.find_vdev(0).unwrap();
        let rv = RprocVirtio::new_driver(v.clone(), shm(), |_: u32| {});
        // Outside shared memory.
        v.set_vring_da(1, 0x1000).unwrap();
        assert!(rv.queue(1).is_err());
        // A ring size that is no power of two.
        let o = v.offset() + 28 + 20 + 8;
        v.io().write_u32(o, 3).unwrap();
        v.set_vring_da(1, VRING0_DA).unwrap();
        assert_eq!(rv.queue(1).unwrap_err(), rustamp_virtio::Error::BadLayout);
        // Too large for a u16.
        v.io().write_u32(o, 1 << 16).unwrap();
        assert_eq!(rv.queue(1).unwrap_err(), rustamp_virtio::Error::BadLayout);
    }
}
