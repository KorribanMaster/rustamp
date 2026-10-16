// SPDX-License-Identifier: BSD-3-Clause
//! Split virtqueues in driver and device roles (`virtqueue.c`).
//!
//! Synchronisation: buffer contents and descriptors are written with plain
//! (volatile) stores, then the ring index is published with a `Release`
//! store; the peer reads the index with `Acquire` before touching anything it
//! covers. Notification decisions use a full `SeqCst` fence, matching
//! `virtio_mb()` in the C implementation.

use core::sync::atomic::{Ordering, fence};

use openamp_metal::IoRegion;

use crate::ring::{
    VRING_AVAIL_F_NO_INTERRUPT, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE, VRING_USED_F_NO_NOTIFY,
    VringLayout, vring_need_event,
};
use crate::{Error, Result};

/// Where a vring lives and how to notify its peer.
#[derive(Debug, Clone)]
pub struct QueueConfig {
    /// Region containing the ring (and, usually, the buffers).
    pub io: IoRegion,
    /// Offset of the ring within `io`.
    pub offset: usize,
    /// Ring geometry.
    pub layout: VringLayout,
    /// Notify id passed to the transport's notifier for this queue.
    pub notify_id: u32,
}

impl QueueConfig {
    /// Physical (device) address of the ring.
    pub fn phys(&self) -> Option<u64> {
        self.io.offset_to_phys(self.offset)
    }
}

/// A descriptor as stored in the descriptor table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Desc {
    /// Physical (device) address of the buffer.
    pub addr: u64,
    /// Length of the buffer.
    pub len: u32,
    /// `VRING_DESC_F_*` flags.
    pub flags: u16,
    /// Next descriptor in the chain if `flags` has `NEXT`.
    pub next: u16,
}

impl Desc {
    /// The device may write this buffer.
    #[inline]
    pub fn is_device_writable(&self) -> bool {
        self.flags & VRING_DESC_F_WRITE != 0
    }

    /// The chain continues.
    #[inline]
    pub fn has_next(&self) -> bool {
        self.flags & VRING_DESC_F_NEXT != 0
    }
}

/// One buffer to post on a [`DriverQueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    /// Physical (device) address.
    pub addr: u64,
    /// Length in bytes.
    pub len: u32,
    /// Whether the device writes it (receive buffer) or reads it (send).
    pub device_writable: bool,
}

/// A buffer chain the device returned to the driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Used {
    /// Head descriptor index of the chain.
    pub head: u16,
    /// Bytes the device reports as written.
    pub len: u32,
    /// Address of the head buffer.
    pub addr: u64,
    /// Length of the head buffer as posted.
    pub buf_len: u32,
}

/// A buffer chain taken from the avail ring by the device.
///
/// Holding an `AvailBuf` is holding the descriptor: it is not `Clone`, and
/// [`DeviceQueue::push_used`] consumes it, so a buffer cannot be returned to
/// the driver twice.
#[derive(Debug, PartialEq, Eq)]
pub struct AvailBuf {
    head: u16,
    desc: Desc,
}

impl AvailBuf {
    /// Head descriptor index.
    #[inline]
    pub fn head(&self) -> u16 {
        self.head
    }

    /// The head descriptor.
    #[inline]
    pub fn desc(&self) -> Desc {
        self.desc
    }
}

struct Ring {
    io: IoRegion,
    base: usize,
    layout: VringLayout,
}

impl Ring {
    fn new(cfg: &QueueConfig) -> Result<Self> {
        cfg.io.check(cfg.offset, cfg.layout.size())?;
        if (cfg.io.as_ptr() as usize + cfg.offset) % 16 != 0 {
            return Err(Error::Metal(openamp_metal::Error::Misaligned));
        }
        Ok(Ring {
            io: cfg.io.clone(),
            base: cfg.offset,
            layout: cfg.layout,
        })
    }

    #[inline]
    fn num(&self) -> u16 {
        self.layout.num()
    }

    #[inline]
    fn slot(&self, idx: u16) -> u16 {
        idx & (self.num() - 1)
    }

    fn read_desc(&self, i: u16) -> Result<Desc> {
        if i >= self.num() {
            return Err(Error::InvalidDescriptor);
        }
        let o = self.base + self.layout.desc(i);
        Ok(Desc {
            addr: u64::from_le(self.io.read_u64(o)?),
            len: u32::from_le(self.io.read_u32(o + 8)?),
            flags: u16::from_le(self.io.read_u16(o + 12)?),
            next: u16::from_le(self.io.read_u16(o + 14)?),
        })
    }

    fn write_desc(&self, i: u16, d: Desc) -> Result<()> {
        let o = self.base + self.layout.desc(i);
        self.io.write_u64(o, d.addr.to_le())?;
        self.io.write_u32(o + 8, d.len.to_le())?;
        self.io.write_u16(o + 12, d.flags.to_le())?;
        self.io.write_u16(o + 14, d.next.to_le())?;
        Ok(())
    }

    fn write_next(&self, i: u16, next: u16) -> Result<()> {
        self.io
            .write_u16(self.base + self.layout.desc(i) + 14, next.to_le())?;
        Ok(())
    }

    #[inline]
    fn r16(&self, off: usize) -> Result<u16> {
        Ok(u16::from_le(self.io.read_u16(self.base + off)?))
    }

    #[inline]
    fn w16(&self, off: usize, v: u16) -> Result<()> {
        Ok(self.io.write_u16(self.base + off, v.to_le())?)
    }

    #[inline]
    fn r32(&self, off: usize) -> Result<u32> {
        Ok(u32::from_le(self.io.read_u32(self.base + off)?))
    }

    #[inline]
    fn w32(&self, off: usize, v: u32) -> Result<()> {
        Ok(self.io.write_u32(self.base + off, v.to_le())?)
    }

    #[inline]
    fn load16(&self, off: usize, order: Ordering) -> Result<u16> {
        Ok(u16::from_le(self.io.load_u16(self.base + off, order)?))
    }

    #[inline]
    fn store16(&self, off: usize, v: u16, order: Ordering) -> Result<()> {
        Ok(self.io.store_u16(self.base + off, v.to_le(), order)?)
    }
}

/// The driver side of a virtqueue: posts buffers, collects them back.
///
/// In rpmsg the driver is the host (Linux, or an OpenAMP host built with this
/// crate). The driver owns the ring memory and initialises it.
pub struct DriverQueue {
    ring: Ring,
    notify_id: u32,
    event_idx: bool,
    free_head: u16,
    num_free: u16,
    avail_shadow: u16,
    kicked_at: u16,
    last_used: u16,
    interrupts: bool,
}

impl DriverQueue {
    /// Initialises the ring described by `cfg` (zeroing it) and takes the
    /// driver role. `event_idx` must match the negotiated
    /// `VIRTIO_RING_F_EVENT_IDX` feature.
    pub fn new(cfg: &QueueConfig, event_idx: bool) -> Result<Self> {
        let ring = Ring::new(cfg)?;
        ring.io.fill(ring.base, 0, ring.layout.size())?;
        let num = ring.num();
        for i in 0..num - 1 {
            ring.write_next(i, i + 1)?;
        }
        Ok(DriverQueue {
            ring,
            notify_id: cfg.notify_id,
            event_idx,
            free_head: 0,
            num_free: num,
            avail_shadow: 0,
            kicked_at: 0,
            last_used: 0,
            interrupts: true,
        })
    }

    /// Ring size.
    pub fn size(&self) -> u16 {
        self.ring.num()
    }

    /// Free descriptors.
    pub fn num_free(&self) -> u16 {
        self.num_free
    }

    /// Notify id of this queue.
    pub fn notify_id(&self) -> u32 {
        self.notify_id
    }

    /// Posts a chain of buffers; returns the head descriptor index.
    pub fn add(&mut self, bufs: &[Buffer]) -> Result<u16> {
        if bufs.is_empty() || bufs.len() > self.ring.num() as usize {
            return Err(Error::Metal(openamp_metal::Error::InvalidParam));
        }
        if bufs.len() > self.num_free as usize {
            return Err(Error::QueueFull);
        }
        let head = self.free_head;
        let mut idx = head;
        for (i, b) in bufs.iter().enumerate() {
            let next = self.ring.read_desc(idx)?.next;
            let mut flags = 0;
            if b.device_writable {
                flags |= VRING_DESC_F_WRITE;
            }
            if i + 1 < bufs.len() {
                flags |= VRING_DESC_F_NEXT;
            }
            self.ring.write_desc(
                idx,
                Desc {
                    addr: b.addr,
                    len: b.len,
                    flags,
                    next,
                },
            )?;
            if i + 1 < bufs.len() || self.num_free as usize > bufs.len() {
                // The free list continues through `next`; it must stay valid.
                if next >= self.ring.num() {
                    return Err(Error::InvalidDescriptor);
                }
            }
            idx = next;
        }
        self.free_head = idx;
        self.num_free -= bufs.len() as u16;

        let slot = self.ring.slot(self.avail_shadow);
        self.ring.w16(self.ring.layout.avail_ring(slot), head)?;
        self.avail_shadow = self.avail_shadow.wrapping_add(1);
        self.ring.store16(
            self.ring.layout.avail_idx(),
            self.avail_shadow,
            Ordering::Release,
        )?;
        Ok(head)
    }

    /// Whether the device has returned buffers not yet collected.
    pub fn has_used(&self) -> Result<bool> {
        Ok(self
            .ring
            .load16(self.ring.layout.used_idx(), Ordering::Acquire)?
            != self.last_used)
    }

    /// Collects the next buffer chain returned by the device.
    pub fn pop_used(&mut self) -> Result<Option<Used>> {
        let num = self.ring.num();
        let used_idx = self
            .ring
            .load16(self.ring.layout.used_idx(), Ordering::Acquire)?;
        if used_idx == self.last_used {
            return Ok(None);
        }
        if used_idx.wrapping_sub(self.last_used) > num {
            return Err(Error::InvalidDescriptor);
        }
        let slot = self.ring.slot(self.last_used);
        let elem = self.ring.layout.used_elem(slot);
        let id = self.ring.r32(elem)?;
        let len = self.ring.r32(elem + 4)?;
        if id >= num as u32 {
            return Err(Error::InvalidDescriptor);
        }
        let head = id as u16;
        let first = self.ring.read_desc(head)?;

        // Return the chain to the free list.
        let mut tail = head;
        let mut d = first;
        let mut count: u16 = 1;
        while d.has_next() {
            if count >= num {
                return Err(Error::InvalidDescriptor);
            }
            tail = d.next;
            d = self.ring.read_desc(tail)?;
            count += 1;
        }
        if self.num_free + count > num {
            return Err(Error::InvalidDescriptor);
        }
        self.ring.write_next(tail, self.free_head)?;
        self.free_head = head;
        self.num_free += count;

        self.last_used = self.last_used.wrapping_add(1);
        if self.event_idx && self.interrupts {
            self.ring.store16(
                self.ring.layout.used_event(),
                self.last_used,
                Ordering::Release,
            )?;
        }
        Ok(Some(Used {
            head,
            len,
            addr: first.addr,
            buf_len: first.len,
        }))
    }

    /// Whether the device must be notified about buffers added since the
    /// last call (`virtqueue_kick` decision).
    pub fn kick_needed(&mut self) -> Result<bool> {
        fence(Ordering::SeqCst);
        let old = self.kicked_at;
        let new = self.avail_shadow;
        self.kicked_at = new;
        if old == new {
            return Ok(false);
        }
        if self.event_idx {
            let ev = self
                .ring
                .load16(self.ring.layout.avail_event(), Ordering::Acquire)?;
            Ok(vring_need_event(ev, new, old))
        } else {
            let flags = self
                .ring
                .load16(self.ring.layout.used_flags(), Ordering::Acquire)?;
            Ok(flags & VRING_USED_F_NO_NOTIFY == 0)
        }
    }

    /// Asks the device not to interrupt when it returns buffers.
    pub fn disable_interrupts(&mut self) -> Result<()> {
        self.interrupts = false;
        if self.event_idx {
            // Park used_event a full ring behind, so it is never crossed
            // (as `virtqueue_disable_cb` does).
            let parked = self.last_used.wrapping_sub(self.ring.num()).wrapping_sub(1);
            self.ring
                .store16(self.ring.layout.used_event(), parked, Ordering::Release)?;
        }
        let f = self.ring.r16(self.ring.layout.avail_flags())?;
        self.ring
            .w16(self.ring.layout.avail_flags(), f | VRING_AVAIL_F_NO_INTERRUPT)
    }

    /// Asks the device to interrupt again. Returns `true` if buffers were
    /// returned meanwhile, so the caller must process them before sleeping.
    pub fn enable_interrupts(&mut self) -> Result<bool> {
        self.interrupts = true;
        let f = self.ring.r16(self.ring.layout.avail_flags())?;
        self.ring
            .w16(self.ring.layout.avail_flags(), f & !VRING_AVAIL_F_NO_INTERRUPT)?;
        if self.event_idx {
            self.ring.store16(
                self.ring.layout.used_event(),
                self.last_used,
                Ordering::Release,
            )?;
        }
        fence(Ordering::SeqCst);
        self.has_used()
    }
}

/// The device side of a virtqueue: takes posted buffers, returns them.
///
/// In rpmsg the device is the remote firmware. The device never writes the
/// descriptor table or the avail ring.
pub struct DeviceQueue {
    ring: Ring,
    notify_id: u32,
    event_idx: bool,
    last_avail: u16,
    used_shadow: u16,
    kicked_at: u16,
    notifications: bool,
}

impl DeviceQueue {
    /// Attaches to the ring described by `cfg`, initialised by the driver.
    pub fn new(cfg: &QueueConfig, event_idx: bool) -> Result<Self> {
        let ring = Ring::new(cfg)?;
        let used = ring.load16(ring.layout.used_idx(), Ordering::Acquire)?;
        Ok(DeviceQueue {
            ring,
            notify_id: cfg.notify_id,
            event_idx,
            last_avail: used,
            used_shadow: used,
            kicked_at: used,
            notifications: true,
        })
    }

    /// Ring size.
    pub fn size(&self) -> u16 {
        self.ring.num()
    }

    /// Notify id of this queue.
    pub fn notify_id(&self) -> u32 {
        self.notify_id
    }

    /// Whether the driver has posted buffers not yet taken.
    pub fn has_avail(&self) -> Result<bool> {
        Ok(self
            .ring
            .load16(self.ring.layout.avail_idx(), Ordering::Acquire)?
            != self.last_avail)
    }

    /// Takes the next posted buffer chain.
    pub fn pop_avail(&mut self) -> Result<Option<AvailBuf>> {
        let num = self.ring.num();
        let avail_idx = self
            .ring
            .load16(self.ring.layout.avail_idx(), Ordering::Acquire)?;
        if avail_idx == self.last_avail {
            return Ok(None);
        }
        if avail_idx.wrapping_sub(self.last_avail) > num {
            return Err(Error::InvalidDescriptor);
        }
        let slot = self.ring.slot(self.last_avail);
        let head = self.ring.r16(self.ring.layout.avail_ring(slot))?;
        let desc = self.ring.read_desc(head)?;
        self.last_avail = self.last_avail.wrapping_add(1);
        if self.event_idx && self.notifications {
            self.ring.store16(
                self.ring.layout.avail_event(),
                self.last_avail,
                Ordering::Release,
            )?;
        }
        Ok(Some(AvailBuf { head, desc }))
    }

    /// Iterates over the descriptors of a chain, head first.
    pub fn chain<'a>(&'a self, buf: &AvailBuf) -> Chain<'a> {
        Chain {
            ring: &self.ring,
            next: Some(buf.desc),
            seen: 0,
        }
    }

    /// Returns a chain to the driver, reporting `len` bytes written.
    pub fn push_used(&mut self, buf: AvailBuf, len: u32) -> Result<()> {
        let slot = self.ring.slot(self.used_shadow);
        let elem = self.ring.layout.used_elem(slot);
        self.ring.w32(elem, buf.head as u32)?;
        self.ring.w32(elem + 4, len)?;
        self.used_shadow = self.used_shadow.wrapping_add(1);
        self.ring.store16(
            self.ring.layout.used_idx(),
            self.used_shadow,
            Ordering::Release,
        )
    }

    /// Whether the driver must be interrupted about buffers returned since
    /// the last call.
    pub fn kick_needed(&mut self) -> Result<bool> {
        fence(Ordering::SeqCst);
        let old = self.kicked_at;
        let new = self.used_shadow;
        self.kicked_at = new;
        if old == new {
            return Ok(false);
        }
        if self.event_idx {
            let ev = self
                .ring
                .load16(self.ring.layout.used_event(), Ordering::Acquire)?;
            Ok(vring_need_event(ev, new, old))
        } else {
            let flags = self
                .ring
                .load16(self.ring.layout.avail_flags(), Ordering::Acquire)?;
            Ok(flags & VRING_AVAIL_F_NO_INTERRUPT == 0)
        }
    }

    /// Asks the driver not to notify when it posts buffers.
    pub fn disable_notifications(&mut self) -> Result<()> {
        self.notifications = false;
        if self.event_idx {
            let parked = self.last_avail.wrapping_sub(self.ring.num()).wrapping_sub(1);
            self.ring
                .store16(self.ring.layout.avail_event(), parked, Ordering::Release)?;
        }
        let f = self.ring.r16(self.ring.layout.used_flags())?;
        self.ring
            .w16(self.ring.layout.used_flags(), f | VRING_USED_F_NO_NOTIFY)
    }

    /// Asks the driver to notify again. Returns `true` if buffers were
    /// posted meanwhile.
    pub fn enable_notifications(&mut self) -> Result<bool> {
        self.notifications = true;
        let f = self.ring.r16(self.ring.layout.used_flags())?;
        self.ring
            .w16(self.ring.layout.used_flags(), f & !VRING_USED_F_NO_NOTIFY)?;
        if self.event_idx {
            self.ring.store16(
                self.ring.layout.avail_event(),
                self.last_avail,
                Ordering::Release,
            )?;
        }
        fence(Ordering::SeqCst);
        self.has_avail()
    }
}

/// Iterator over a descriptor chain; see [`DeviceQueue::chain`].
pub struct Chain<'a> {
    ring: &'a Ring,
    next: Option<Desc>,
    seen: u16,
}

impl Iterator for Chain<'_> {
    type Item = Result<Desc>;

    fn next(&mut self) -> Option<Self::Item> {
        let d = self.next.take()?;
        self.seen += 1;
        if d.has_next() {
            if self.seen >= self.ring.num() {
                return Some(Err(Error::InvalidDescriptor));
            }
            match self.ring.read_desc(d.next) {
                Ok(n) => self.next = Some(n),
                Err(e) => return Some(Err(e)),
            }
        }
        Some(Ok(d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn pair(num: u16, event_idx: bool) -> (DriverQueue, DeviceQueue, IoRegion) {
        let layout = VringLayout::new(num, 64).unwrap();
        let io = IoRegion::alloc(layout.size() + 0x1000, 0x4000_0000);
        let cfg = QueueConfig {
            io: io.clone(),
            offset: 0,
            layout,
            notify_id: 3,
        };
        let drv = DriverQueue::new(&cfg, event_idx).unwrap();
        let dev = DeviceQueue::new(&cfg, event_idx).unwrap();
        (drv, dev, io)
    }

    fn buf(addr: u64, len: u32) -> Buffer {
        Buffer {
            addr,
            len,
            device_writable: true,
        }
    }

    #[test]
    fn round_trip_single_buffers() {
        let (mut drv, mut dev, _) = pair(4, false);
        for round in 0..10u64 {
            for i in 0..4 {
                drv.add(&[buf(0x1000 * i + round, 64)]).unwrap();
            }
            assert_eq!(drv.add(&[buf(0, 1)]), Err(Error::QueueFull));
            assert!(drv.kick_needed().unwrap());
            let mut got = Vec::new();
            while let Some(b) = dev.pop_avail().unwrap() {
                assert_eq!(b.desc().len, 64);
                assert!(b.desc().is_device_writable());
                got.push(b.desc().addr);
                dev.push_used(b, 7).unwrap();
            }
            assert_eq!(got.len(), 4);
            assert!(dev.kick_needed().unwrap());
            let mut n = 0;
            while let Some(u) = drv.pop_used().unwrap() {
                assert_eq!(u.len, 7);
                assert_eq!(u.addr, got[n]);
                n += 1;
            }
            assert_eq!(n, 4);
            assert_eq!(drv.num_free(), 4);
        }
    }

    #[test]
    fn chains_are_walked_and_freed() {
        let (mut drv, mut dev, _) = pair(8, false);
        let head = drv
            .add(&[
                Buffer {
                    addr: 1,
                    len: 10,
                    device_writable: false,
                },
                buf(2, 20),
                buf(3, 30),
            ])
            .unwrap();
        assert_eq!(drv.num_free(), 5);
        let b = dev.pop_avail().unwrap().unwrap();
        assert_eq!(b.head(), head);
        let lens: Vec<u32> = dev.chain(&b).map(|d| d.unwrap().len).collect();
        assert_eq!(lens, [10, 20, 30]);
        dev.push_used(b, 50).unwrap();
        let u = drv.pop_used().unwrap().unwrap();
        assert_eq!((u.head, u.len, u.addr), (head, 50, 1));
        assert_eq!(drv.num_free(), 8);
        // All eight descriptors are usable again.
        for i in 0..8 {
            drv.add(&[buf(i, 1)]).unwrap();
        }
    }

    #[test]
    fn notification_suppression_flags() {
        let (mut drv, mut dev, _) = pair(4, false);
        dev.disable_notifications().unwrap();
        drv.add(&[buf(1, 1)]).unwrap();
        assert!(!drv.kick_needed().unwrap());
        assert!(dev.enable_notifications().unwrap()); // pending
        drv.add(&[buf(2, 1)]).unwrap();
        assert!(drv.kick_needed().unwrap());

        drv.disable_interrupts().unwrap();
        let b = dev.pop_avail().unwrap().unwrap();
        dev.push_used(b, 0).unwrap();
        assert!(!dev.kick_needed().unwrap());
        assert!(drv.enable_interrupts().unwrap());
        let b = dev.pop_avail().unwrap().unwrap();
        dev.push_used(b, 0).unwrap();
        assert!(dev.kick_needed().unwrap());
    }

    #[test]
    fn event_idx_suppression() {
        let (mut drv, mut dev, _) = pair(8, true);
        // Device has not consumed anything: avail_event = 0, so the first
        // add (0 -> 1) crosses it and needs a kick, the second does not.
        drv.add(&[buf(1, 1)]).unwrap();
        assert!(drv.kick_needed().unwrap());
        drv.add(&[buf(2, 1)]).unwrap();
        assert!(!drv.kick_needed().unwrap());
        // Device consumes both, re-arming avail_event = 2.
        let a = dev.pop_avail().unwrap().unwrap();
        let b = dev.pop_avail().unwrap().unwrap();
        drv.add(&[buf(3, 1)]).unwrap();
        assert!(drv.kick_needed().unwrap());
        // Driver's used_event = 0 initially: first returned buffer interrupts.
        dev.push_used(a, 0).unwrap();
        assert!(dev.kick_needed().unwrap());
        dev.push_used(b, 0).unwrap();
        assert!(!dev.kick_needed().unwrap());
        // Disabled interrupts park used_event: no kicks however far we go.
        drv.disable_interrupts().unwrap();
        while drv.pop_used().unwrap().is_some() {}
        let c = dev.pop_avail().unwrap().unwrap();
        dev.push_used(c, 0).unwrap();
        assert!(!dev.kick_needed().unwrap());
        // Re-enabling reports the pending buffer.
        assert!(drv.enable_interrupts().unwrap());
        // Device side parking works the same way.
        dev.disable_notifications().unwrap();
        drv.add(&[buf(4, 1)]).unwrap();
        assert!(!drv.kick_needed().unwrap());
        assert!(dev.enable_notifications().unwrap());
    }

    #[test]
    fn hostile_indices_are_rejected() {
        let (mut drv, mut dev, io) = pair(4, false);
        let l = VringLayout::new(4, 64).unwrap();
        // Driver publishes a head outside the table.
        io.write_u16(l.avail_ring(0), 9).unwrap();
        io.store_u16(l.avail_idx(), 1, Ordering::Release).unwrap();
        assert_eq!(dev.pop_avail(), Err(Error::InvalidDescriptor));
        // Device jumps used.idx by more than the ring size.
        io.store_u16(l.used_idx(), 100, Ordering::Release).unwrap();
        assert_eq!(drv.pop_used(), Err(Error::InvalidDescriptor));
        // Device returns an id outside the table.
        io.store_u16(l.used_idx(), 1, Ordering::Release).unwrap();
        io.write_u32(l.used_elem(0), 77).unwrap();
        assert_eq!(drv.pop_used(), Err(Error::InvalidDescriptor));
    }

    #[test]
    fn looping_chain_is_detected() {
        let (_, mut dev, io) = pair(4, false);
        let l = VringLayout::new(4, 64).unwrap();
        // desc0 -> desc1 -> desc0 ...
        for (i, next) in [(0u16, 1u16), (1, 0)] {
            io.write_u16(l.desc(i) + 12, VRING_DESC_F_NEXT).unwrap();
            io.write_u16(l.desc(i) + 14, next).unwrap();
        }
        io.store_u16(l.avail_idx(), 1, Ordering::Release).unwrap();
        let b = dev.pop_avail().unwrap().unwrap();
        assert!(dev.chain(&b).any(|d| d.is_err()));
    }

    #[test]
    fn threads_exchange_a_million_buffers() {
        let (mut drv, mut dev, _) = pair(16, true);
        const N: u64 = 200_000;
        let h = std::thread::spawn(move || {
            let mut seen = 0u64;
            while seen < N {
                if let Some(b) = dev.pop_avail().unwrap() {
                    assert_eq!(b.desc().addr, seen);
                    dev.push_used(b, seen as u32).unwrap();
                    seen += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        });
        let (mut sent, mut done) = (0u64, 0u64);
        while done < N {
            while sent < N && drv.num_free() > 0 {
                drv.add(&[buf(sent, 1)]).unwrap();
                sent += 1;
            }
            while let Some(u) = drv.pop_used().unwrap() {
                assert_eq!(u.len as u64, done);
                done += 1;
            }
        }
        h.join().unwrap();
    }
}
