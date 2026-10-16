// SPDX-License-Identifier: BSD-3-Clause
//! Split virtqueue ("vring") layout.
//!
//! ```text
//! offset 0          descriptor table   num × { addr: u64, len: u32, flags: u16, next: u16 }
//! 16·num            avail ring         flags: u16, idx: u16, ring[num]: u16, used_event: u16
//! align_up(…,align) used ring          flags: u16, idx: u16, ring[num]: { id: u32, len: u32 },
//!                                      avail_event: u16
//! ```
//!
//! This is `vring_init()`/`vring_size()` from `open-amp/virtio_ring.h` and the
//! Linux kernel. All fields are little-endian.

use crate::{Error, Result};

/// Descriptor continues in `next`.
pub const VRING_DESC_F_NEXT: u16 = 1;
/// Buffer is device write-only (otherwise device read-only).
pub const VRING_DESC_F_WRITE: u16 = 2;
/// Buffer contains a list of descriptors.
pub const VRING_DESC_F_INDIRECT: u16 = 4;
/// Driver → device: don't interrupt me when you consume buffers.
pub const VRING_AVAIL_F_NO_INTERRUPT: u16 = 1;
/// Device → driver: don't notify me when you add buffers.
pub const VRING_USED_F_NO_NOTIFY: u16 = 1;

/// Size of one descriptor.
pub const DESC_SIZE: usize = 16;
/// Size of one used-ring element.
pub const USED_ELEM_SIZE: usize = 8;

/// The geometry of one vring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VringLayout {
    num: u16,
    align: usize,
}

impl VringLayout {
    /// A ring of `num` entries (a power of two, at most 32768) whose used
    /// ring starts on an `align` boundary (a power of two, at least 4).
    pub const fn new(num: u16, align: usize) -> Result<Self> {
        if num == 0 || !num.is_power_of_two() || num > 32768 {
            return Err(Error::BadLayout);
        }
        if align < 4 || !align.is_power_of_two() {
            return Err(Error::BadLayout);
        }
        Ok(VringLayout { num, align })
    }

    /// Number of entries.
    #[inline]
    pub const fn num(&self) -> u16 {
        self.num
    }

    /// Used-ring alignment.
    #[inline]
    pub const fn align(&self) -> usize {
        self.align
    }

    /// Offset of descriptor `i`.
    #[inline]
    pub const fn desc(&self, i: u16) -> usize {
        DESC_SIZE * i as usize
    }

    /// Offset of the avail ring.
    #[inline]
    pub const fn avail(&self) -> usize {
        DESC_SIZE * self.num as usize
    }

    /// Offset of `avail.flags`.
    #[inline]
    pub const fn avail_flags(&self) -> usize {
        self.avail()
    }

    /// Offset of `avail.idx`.
    #[inline]
    pub const fn avail_idx(&self) -> usize {
        self.avail() + 2
    }

    /// Offset of `avail.ring[i]`.
    #[inline]
    pub const fn avail_ring(&self, i: u16) -> usize {
        self.avail() + 4 + 2 * i as usize
    }

    /// Offset of `used_event` (after the avail ring).
    #[inline]
    pub const fn used_event(&self) -> usize {
        self.avail() + 4 + 2 * self.num as usize
    }

    /// Offset of the used ring.
    #[inline]
    pub const fn used(&self) -> usize {
        openamp_metal::align_up(self.avail() + 6 + 2 * self.num as usize, self.align)
    }

    /// Offset of `used.flags`.
    #[inline]
    pub const fn used_flags(&self) -> usize {
        self.used()
    }

    /// Offset of `used.idx`.
    #[inline]
    pub const fn used_idx(&self) -> usize {
        self.used() + 2
    }

    /// Offset of `used.ring[i]`.
    #[inline]
    pub const fn used_elem(&self, i: u16) -> usize {
        self.used() + 4 + USED_ELEM_SIZE * i as usize
    }

    /// Offset of `avail_event` (after the used ring).
    #[inline]
    pub const fn avail_event(&self) -> usize {
        self.used() + 4 + USED_ELEM_SIZE * self.num as usize
    }

    /// Total size in bytes (`vring_size()`).
    #[inline]
    pub const fn size(&self) -> usize {
        self.used() + 6 + USED_ELEM_SIZE * self.num as usize
    }
}

/// `vring_need_event()`: whether moving an index from `old` to `new` crosses
/// the peer's requested `event_idx`.
#[inline]
pub const fn vring_need_event(event_idx: u16, new: u16, old: u16) -> bool {
    new.wrapping_sub(event_idx).wrapping_sub(1) < new.wrapping_sub(old)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_c_vring_size() {
        // vring_size(num, align) from virtio_ring.h, computed by hand.
        fn c_vring_size(num: usize, align: usize) -> usize {
            let mut size = num * 16;
            size += 2 * (3 + num);
            size = (size + align - 1) & !(align - 1);
            size += 2 * 3 + 8 * num;
            size
        }
        for &(num, align) in &[(1, 4), (16, 16), (256, 4096), (512, 4096), (64, 64)] {
            let l = VringLayout::new(num as u16, align).unwrap();
            assert_eq!(
                l.size(),
                c_vring_size(num, align),
                "num={num} align={align}"
            );
        }
        let l = VringLayout::new(256, 4096).unwrap();
        assert_eq!(l.avail(), 4096);
        assert_eq!(l.used(), 8192);
        assert_eq!(l.used_event(), 4096 + 4 + 512);
        assert_eq!(l.avail_event(), 8192 + 4 + 2048);
    }

    #[test]
    fn rejects_bad_geometry() {
        assert_eq!(VringLayout::new(0, 4096), Err(Error::BadLayout));
        assert_eq!(VringLayout::new(3, 4096), Err(Error::BadLayout));
        assert_eq!(VringLayout::new(4, 2), Err(Error::BadLayout));
        assert_eq!(VringLayout::new(4, 24), Err(Error::BadLayout));
    }

    #[test]
    fn need_event_wraps() {
        assert!(vring_need_event(0, 1, 0));
        assert!(!vring_need_event(5, 3, 0));
        assert!(vring_need_event(0xffff, 1, 0xfffe));
        assert!(!vring_need_event(10, 10, 10));
    }
}
