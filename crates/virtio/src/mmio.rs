// SPDX-License-Identifier: BSD-3-Clause
//! virtio-mmio transport, driver side (`virtio_mmio_drv.c`).
//!
//! Supports register layout version 1 (legacy, page-frame queue addresses)
//! and version 2 (split 64-bit queue addresses).

use core::sync::atomic::{AtomicU32, Ordering};

use openamp_metal::IoRegion;
use openamp_metal::sync::SpinMutex;

use crate::{Error, QueueConfig, Result, Role, Transport};

/// Register offsets.
#[allow(missing_docs)]
pub mod regs {
    pub const MAGIC: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const VENDOR_ID: usize = 0x00c;
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    pub const GUEST_PAGE_SIZE: usize = 0x028;
    pub const QUEUE_SEL: usize = 0x030;
    pub const QUEUE_NUM_MAX: usize = 0x034;
    pub const QUEUE_NUM: usize = 0x038;
    pub const QUEUE_ALIGN: usize = 0x03c;
    pub const QUEUE_PFN: usize = 0x040;
    pub const QUEUE_READY: usize = 0x044;
    pub const QUEUE_NOTIFY: usize = 0x050;
    pub const INTERRUPT_STATUS: usize = 0x060;
    pub const INTERRUPT_ACK: usize = 0x064;
    pub const STATUS: usize = 0x070;
    pub const QUEUE_DESC_LOW: usize = 0x080;
    pub const QUEUE_DESC_HIGH: usize = 0x084;
    pub const QUEUE_AVAIL_LOW: usize = 0x090;
    pub const QUEUE_AVAIL_HIGH: usize = 0x094;
    pub const QUEUE_USED_LOW: usize = 0x0a0;
    pub const QUEUE_USED_HIGH: usize = 0x0a4;
    pub const CONFIG: usize = 0x100;
}

/// `"virt"` in little-endian.
pub const MAGIC_VALUE: u32 = 0x7472_6976;
/// Page size used with legacy (version 1) devices.
pub const LEGACY_PAGE_SIZE: u32 = 4096;
/// Maximum number of queues this transport tracks.
pub const MAX_QUEUES: usize = 8;

/// A virtio-mmio device, driven from this side.
pub struct MmioTransport<N: Fn(usize) = fn(usize)> {
    regs: IoRegion,
    version: u32,
    driver_features: [AtomicU32; 2],
    queues: SpinMutex<[Option<QueueConfig>; MAX_QUEUES]>,
    after_notify: Option<N>,
}

impl MmioTransport {
    /// Probes the register block at `regs`.
    pub fn new(regs: IoRegion) -> Result<Self> {
        Self::with_hook(regs, None)
    }
}

impl<N: Fn(usize)> MmioTransport<N> {
    /// Probes the register block; `after_notify` runs after each queue
    /// notification (useful when the "device" is software on another core
    /// that needs an extra doorbell).
    pub fn with_hook(regs: IoRegion, after_notify: Option<N>) -> Result<Self> {
        regs.check(0, regs::CONFIG)?;
        if regs.read_u32(regs::MAGIC)? != MAGIC_VALUE {
            return Err(Error::NotReady);
        }
        let version = regs.read_u32(regs::VERSION)?;
        if version != 1 && version != 2 {
            return Err(Error::Unsupported);
        }
        if regs.read_u32(regs::DEVICE_ID)? == 0 {
            // Placeholder device: nothing behind this slot.
            return Err(Error::NotReady);
        }
        if version == 1 {
            regs.write_u32(regs::GUEST_PAGE_SIZE, LEGACY_PAGE_SIZE)?;
        }
        Ok(MmioTransport {
            regs,
            version,
            driver_features: [AtomicU32::new(0), AtomicU32::new(0)],
            queues: SpinMutex::new([const { None }; MAX_QUEUES]),
            after_notify,
        })
    }

    /// Register layout version (1 = legacy, 2 = modern).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Vendor id.
    pub fn vendor_id(&self) -> u32 {
        self.regs.read_u32(regs::VENDOR_ID).unwrap_or(0)
    }

    /// Largest ring the device accepts for queue `index` (0 = queue absent).
    pub fn max_queue_size(&self, index: usize) -> Result<u32> {
        self.regs.write_u32(regs::QUEUE_SEL, index as u32)?;
        Ok(self.regs.read_u32(regs::QUEUE_NUM_MAX)?)
    }

    /// Pending interrupt causes (bit 0: used buffer, bit 1: config change).
    pub fn interrupt_status(&self) -> u32 {
        self.regs.read_u32(regs::INTERRUPT_STATUS).unwrap_or(0)
    }

    /// Acknowledges interrupt causes.
    pub fn interrupt_ack(&self, bits: u32) {
        let _ = self.regs.write_u32(regs::INTERRUPT_ACK, bits);
    }
}

impl<N: Fn(usize)> Transport for MmioTransport<N> {
    fn role(&self) -> Role {
        Role::Driver
    }

    fn device_id(&self) -> u32 {
        self.regs.read_u32(regs::DEVICE_ID).unwrap_or(0)
    }

    fn status(&self) -> u8 {
        self.regs.read_u32(regs::STATUS).unwrap_or(0) as u8
    }

    fn set_status(&self, status: u8) {
        let _ = self.regs.write_u32(regs::STATUS, status as u32);
    }

    fn device_features(&self) -> u64 {
        let mut f = 0u64;
        for sel in 0..2u32 {
            if self.regs.write_u32(regs::DEVICE_FEATURES_SEL, sel).is_err() {
                return 0;
            }
            let v = self.regs.read_u32(regs::DEVICE_FEATURES).unwrap_or(0) as u64;
            f |= v << (32 * sel);
        }
        f
    }

    fn driver_features(&self) -> u64 {
        self.driver_features[0].load(Ordering::Relaxed) as u64
            | (self.driver_features[1].load(Ordering::Relaxed) as u64) << 32
    }

    fn set_driver_features(&self, features: u64) -> Result<()> {
        for sel in 0..2u32 {
            let v = (features >> (32 * sel)) as u32;
            self.regs.write_u32(regs::DRIVER_FEATURES_SEL, sel)?;
            self.regs.write_u32(regs::DRIVER_FEATURES, v)?;
            self.driver_features[sel as usize].store(v, Ordering::Relaxed);
        }
        Ok(())
    }

    fn read_config(&self, offset: usize, buf: &mut [u8]) -> Result<()> {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.regs.read_u8(regs::CONFIG + offset + i)?;
        }
        Ok(())
    }

    fn write_config(&self, offset: usize, buf: &[u8]) -> Result<()> {
        for (i, b) in buf.iter().enumerate() {
            self.regs.write_u8(regs::CONFIG + offset + i, *b)?;
        }
        Ok(())
    }

    fn queue_count(&self) -> usize {
        (0..MAX_QUEUES)
            .take_while(|&i| self.max_queue_size(i).unwrap_or(0) != 0)
            .count()
    }

    fn queue(&self, index: usize) -> Result<QueueConfig> {
        self.queues
            .lock()
            .get(index)
            .cloned()
            .flatten()
            .ok_or(Error::NotReady)
    }

    fn setup_queue(&self, index: usize, cfg: &QueueConfig) -> Result<()> {
        if index >= MAX_QUEUES {
            return Err(Error::NoSuchQueue);
        }
        let max = self.max_queue_size(index)?;
        if max == 0 {
            return Err(Error::NoSuchQueue);
        }
        let num = cfg.layout.num() as u32;
        if num > max {
            return Err(Error::Rejected);
        }
        let phys = cfg.phys().ok_or(Error::BadLayout)?;
        let r = &self.regs;
        r.write_u32(regs::QUEUE_SEL, index as u32)?;
        r.write_u32(regs::QUEUE_NUM, num)?;
        if self.version == 1 {
            if phys % LEGACY_PAGE_SIZE as u64 != 0 {
                return Err(Error::BadLayout);
            }
            let pfn =
                u32::try_from(phys / LEGACY_PAGE_SIZE as u64).map_err(|_| Error::BadLayout)?;
            r.write_u32(regs::QUEUE_ALIGN, cfg.layout.align() as u32)?;
            r.write_u32(regs::QUEUE_PFN, pfn)?;
        } else {
            let l = &cfg.layout;
            for (lo, addr) in [
                (regs::QUEUE_DESC_LOW, phys),
                (regs::QUEUE_AVAIL_LOW, phys + l.avail() as u64),
                (regs::QUEUE_USED_LOW, phys + l.used() as u64),
            ] {
                r.write_u32(lo, addr as u32)?;
                r.write_u32(lo + 4, (addr >> 32) as u32)?;
            }
            r.write_u32(regs::QUEUE_READY, 1)?;
        }
        self.queues.lock()[index] = Some(cfg.clone());
        Ok(())
    }

    fn notify(&self, index: usize) {
        let _ = self.regs.write_u32(regs::QUEUE_NOTIFY, index as u32);
        if let Some(h) = &self.after_notify {
            h(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::VringLayout;

    fn fake_device(version: u32) -> IoRegion {
        let r = IoRegion::alloc(0x200, 0x0a00_0000);
        r.write_u32(regs::MAGIC, MAGIC_VALUE).unwrap();
        r.write_u32(regs::VERSION, version).unwrap();
        r.write_u32(regs::DEVICE_ID, crate::VIRTIO_ID_RPMSG)
            .unwrap();
        r.write_u32(regs::DEVICE_FEATURES, 1).unwrap();
        r.write_u32(regs::QUEUE_NUM_MAX, 256).unwrap();
        r.write_u8(regs::CONFIG + 2, 0x42).unwrap();
        r
    }

    fn ring_cfg(phys: u64) -> QueueConfig {
        QueueConfig {
            io: IoRegion::alloc(0x4000, phys),
            offset: 0,
            layout: VringLayout::new(64, 4096).unwrap(),
            notify_id: 0,
        }
    }

    #[test]
    fn probe_rejects_non_virtio() {
        let r = IoRegion::alloc(0x200, 0);
        assert!(matches!(MmioTransport::new(r), Err(Error::NotReady)));
    }

    #[test]
    fn modern_queue_setup_programs_addresses() {
        let r = fake_device(2);
        let t = MmioTransport::new(r.clone()).unwrap();
        assert_eq!(t.device_id(), crate::VIRTIO_ID_RPMSG);
        assert_eq!(t.device_features(), 1 | 1 << 32); // fake ignores SEL
        t.set_driver_features(1).unwrap();
        assert_eq!(t.driver_features(), 1);
        let mut c = [0u8; 1];
        t.read_config(2, &mut c).unwrap();
        assert_eq!(c[0], 0x42);

        let cfg = ring_cfg(0x1_8000_0000);
        t.setup_queue(1, &cfg).unwrap();
        assert_eq!(r.read_u32(regs::QUEUE_SEL), Ok(1));
        assert_eq!(r.read_u32(regs::QUEUE_NUM), Ok(64));
        assert_eq!(r.read_u32(regs::QUEUE_DESC_LOW), Ok(0x8000_0000));
        assert_eq!(r.read_u32(regs::QUEUE_DESC_HIGH), Ok(1));
        assert_eq!(r.read_u32(regs::QUEUE_AVAIL_LOW), Ok(0x8000_0400));
        assert_eq!(r.read_u32(regs::QUEUE_USED_LOW), Ok(0x8000_1000));
        assert_eq!(r.read_u32(regs::QUEUE_READY), Ok(1));
        assert_eq!(t.queue(1).unwrap().layout.num(), 64);
        assert!(matches!(t.queue(0), Err(Error::NotReady)));
        t.notify(1);
        assert_eq!(r.read_u32(regs::QUEUE_NOTIFY), Ok(1));
    }

    #[test]
    fn legacy_queue_setup_uses_pfn() {
        let r = fake_device(1);
        let t = MmioTransport::new(r.clone()).unwrap();
        assert_eq!(r.read_u32(regs::GUEST_PAGE_SIZE), Ok(4096));
        t.setup_queue(0, &ring_cfg(0x4000_0000)).unwrap();
        assert_eq!(r.read_u32(regs::QUEUE_PFN), Ok(0x40000));
        assert_eq!(r.read_u32(regs::QUEUE_ALIGN), Ok(4096));
        assert!(matches!(
            t.setup_queue(0, &ring_cfg(0x4000_0100)),
            Err(Error::BadLayout)
        ));
    }
}
