// SPDX-License-Identifier: BSD-3-Clause
//! Shared-memory and MMIO regions (`metal_io_region`).

use core::ptr::NonNull;
use core::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, Ordering, fence};

use crate::{Error, Result};

/// A contiguous span of memory shared with another processor or device.
///
/// `IoRegion` is a cheap, cloneable handle: a virtual base pointer, the
/// physical (device) address that base corresponds to, and a size. Physical
/// addresses are mapped linearly, which covers carve-outs, TCMs and UIO maps.
///
/// All accessors are bounds-checked and return [`Error::OutOfBounds`] instead
/// of touching memory outside the region, so values read from the peer (ring
/// indices, buffer addresses, lengths) can be passed in without first being
/// trusted.
///
/// Access flavours:
///
/// * `read_*`/`write_*`: volatile, single-copy accesses for registers and
///   individual fields.
/// * `load_*`/`store_*`: atomic accesses with an explicit [`Ordering`], used
///   to publish ring indices so that the data written before them becomes
///   visible to the peer first.
/// * [`read_bytes`](Self::read_bytes)/[`write_bytes`](Self::write_bytes):
///   block copies for message payloads. Use them on memory, not on registers.
#[derive(Clone)]
pub struct IoRegion {
    virt: NonNull<u8>,
    phys: u64,
    size: usize,
}

// SAFETY: an IoRegion is a description of memory that is shared by design.
// Every access goes through volatile or atomic operations, and cross-thread
// visibility of block copies is established by the release/acquire index
// protocol of the layers above.
unsafe impl Send for IoRegion {}
// SAFETY: see `Send` above; the handle itself is immutable.
unsafe impl Sync for IoRegion {}

impl core::fmt::Debug for IoRegion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IoRegion")
            .field("virt", &self.virt)
            .field("phys", &format_args!("{:#x}", self.phys))
            .field("size", &format_args!("{:#x}", self.size))
            .finish()
    }
}

macro_rules! volatile_rw {
    ($($read:ident, $write:ident, $ty:ty;)*) => {$(
        #[doc = concat!("Volatile read of a `", stringify!($ty), "` at `offset`.")]
        #[inline]
        pub fn $read(&self, offset: usize) -> Result<$ty> {
            let p = self.aligned_ptr::<$ty>(offset)?;
            // SAFETY: `aligned_ptr` checked bounds and alignment; the region
            // is valid for reads per the contract of `IoRegion::new`.
            Ok(unsafe { p.read_volatile() })
        }

        #[doc = concat!("Volatile write of a `", stringify!($ty), "` at `offset`.")]
        #[inline]
        pub fn $write(&self, offset: usize, value: $ty) -> Result<()> {
            let p = self.aligned_ptr::<$ty>(offset)?;
            // SAFETY: as for the read above; the region is writable.
            unsafe { p.write_volatile(value) };
            Ok(())
        }
    )*};
}

macro_rules! atomic_rw {
    ($($load:ident, $store:ident, $ty:ty, $atomic:ty;)*) => {$(
        #[doc = concat!("Atomic load of a `", stringify!($ty), "` at `offset` with `order`.")]
        #[inline]
        pub fn $load(&self, offset: usize, order: Ordering) -> Result<$ty> {
            let p = self.aligned_ptr::<$ty>(offset)?;
            // SAFETY: the pointer is in bounds, aligned for the atomic type,
            // and valid for the lifetime of the region. All concurrent
            // accesses to ring indices go through these atomic operations.
            Ok(unsafe { <$atomic>::from_ptr(p) }.load(order))
        }

        #[doc = concat!("Atomic store of a `", stringify!($ty), "` at `offset` with `order`.")]
        #[inline]
        pub fn $store(&self, offset: usize, value: $ty, order: Ordering) -> Result<()> {
            let p = self.aligned_ptr::<$ty>(offset)?;
            // SAFETY: as for the load above.
            unsafe { <$atomic>::from_ptr(p) }.store(value, order);
            Ok(())
        }
    )*};
}

impl IoRegion {
    /// Describes `size` bytes at `virt`, which the peer sees at device
    /// address `phys`.
    ///
    /// # Safety
    ///
    /// * `virt..virt + size` must be valid for reads and writes for as long as
    ///   this handle or any clone or subregion of it exists.
    /// * The memory must not be accessed through Rust references while it is
    ///   accessed through the region (shared memory is accessed only through
    ///   `IoRegion`s).
    ///
    /// # Panics
    ///
    /// If `virt` is null or `phys + size` overflows.
    pub unsafe fn new(virt: *mut u8, phys: u64, size: usize) -> Self {
        let virt = NonNull::new(virt).expect("IoRegion base must not be null");
        assert!(
            phys.checked_add(size as u64).is_some(),
            "IoRegion physical range overflows"
        );
        IoRegion { virt, phys, size }
    }

    /// Allocates a zeroed, page-aligned region on the heap and leaks it.
    ///
    /// Intended for host-side simulation and tests, where two "processors"
    /// are two threads sharing this memory.
    #[cfg(feature = "alloc")]
    pub fn alloc(size: usize, phys: u64) -> Self {
        let layout =
            core::alloc::Layout::from_size_align(size.max(1), 4096).expect("invalid IoRegion size");
        // SAFETY: the layout has a non-zero size.
        let p = unsafe { alloc::alloc::alloc_zeroed(layout) };
        if p.is_null() {
            alloc::alloc::handle_alloc_error(layout);
        }
        // The leak is intentional; tell Miri's leak checker so.
        #[cfg(miri)]
        {
            unsafe extern "Rust" {
                fn miri_static_root(ptr: *const u8);
            }
            // SAFETY: Miri-provided hook; `p` is a live heap allocation.
            unsafe { miri_static_root(p) };
        }
        // SAFETY: freshly allocated, zeroed, never freed (leaked), and only
        // ever accessed through this region.
        unsafe { IoRegion::new(p, phys, size) }
    }

    /// Size of the region in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.size
    }

    /// Whether the region is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Physical (device) address of offset 0.
    #[inline]
    pub fn phys(&self) -> u64 {
        self.phys
    }

    /// Virtual address of offset 0.
    #[inline]
    pub fn as_ptr(&self) -> *mut u8 {
        self.virt.as_ptr()
    }

    /// Checks that `offset..offset + len` lies inside the region.
    #[inline]
    pub fn check(&self, offset: usize, len: usize) -> Result<()> {
        match offset.checked_add(len) {
            Some(end) if end <= self.size => Ok(()),
            _ => Err(Error::OutOfBounds),
        }
    }

    #[inline]
    fn ptr(&self, offset: usize, len: usize) -> Result<*mut u8> {
        self.check(offset, len)?;
        // SAFETY: offset <= size, so the result stays within (or one past)
        // the allocation described by the region.
        Ok(unsafe { self.virt.as_ptr().add(offset) })
    }

    #[inline]
    fn aligned_ptr<T>(&self, offset: usize) -> Result<*mut T> {
        let p = self.ptr(offset, core::mem::size_of::<T>())?;
        if (p as usize) % core::mem::align_of::<T>() != 0 {
            return Err(Error::Misaligned);
        }
        Ok(p.cast())
    }

    volatile_rw! {
        read_u8, write_u8, u8;
        read_u16, write_u16, u16;
        read_u32, write_u32, u32;
        read_u64, write_u64, u64;
    }

    atomic_rw! {
        load_u8, store_u8, u8, AtomicU8;
        load_u16, store_u16, u16, AtomicU16;
        load_u32, store_u32, u32, AtomicU32;
    }

    /// Copies `dst.len()` bytes starting at `offset` into `dst`.
    ///
    /// Issues an acquire fence first, so data published by the peer before an
    /// index update observed with `Acquire` is read in full.
    pub fn read_bytes(&self, offset: usize, dst: &mut [u8]) -> Result<()> {
        let p = self.ptr(offset, dst.len())?;
        fence(Ordering::Acquire);
        // SAFETY: bounds checked; `dst` is a distinct Rust allocation, so the
        // ranges cannot overlap.
        unsafe { core::ptr::copy_nonoverlapping(p, dst.as_mut_ptr(), dst.len()) };
        Ok(())
    }

    /// Copies `src` into the region starting at `offset`, followed by a
    /// release fence.
    pub fn write_bytes(&self, offset: usize, src: &[u8]) -> Result<()> {
        let p = self.ptr(offset, src.len())?;
        // SAFETY: bounds checked; `src` cannot overlap shared memory accessed
        // only through regions.
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), p, src.len()) };
        fence(Ordering::Release);
        Ok(())
    }

    /// Fills `len` bytes starting at `offset` with `value`.
    pub fn fill(&self, offset: usize, value: u8, len: usize) -> Result<()> {
        let p = self.ptr(offset, len)?;
        // SAFETY: bounds checked.
        unsafe { core::ptr::write_bytes(p, value, len) };
        fence(Ordering::Release);
        Ok(())
    }

    /// Borrows `len` bytes at `offset` as a slice.
    ///
    /// # Safety
    ///
    /// For the lifetime of the slice nobody — neither this processor nor the
    /// peer — may write the range. In the virtio protocol this holds for a
    /// buffer while its descriptor is owned by this side.
    pub unsafe fn as_slice(&self, offset: usize, len: usize) -> Result<&[u8]> {
        let p = self.ptr(offset, len)?;
        fence(Ordering::Acquire);
        // SAFETY: bounds checked; exclusivity is the caller's obligation.
        Ok(unsafe { core::slice::from_raw_parts(p, len) })
    }

    /// Borrows `len` bytes at `offset` as a mutable slice.
    ///
    /// # Safety
    ///
    /// For the lifetime of the slice nothing else may read or write the
    /// range, and no other slice of it may exist.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn as_mut_slice(&self, offset: usize, len: usize) -> Result<&mut [u8]> {
        let p = self.ptr(offset, len)?;
        // SAFETY: bounds checked; exclusivity is the caller's obligation.
        Ok(unsafe { core::slice::from_raw_parts_mut(p, len) })
    }

    /// Translates a physical address to an offset into the region.
    pub fn phys_to_offset(&self, phys: u64) -> Option<usize> {
        let off = phys.checked_sub(self.phys)?;
        if off < self.size as u64 {
            Some(off as usize)
        } else {
            None
        }
    }

    /// Translates a physical range to an offset, checking that all `len`
    /// bytes are inside the region.
    pub fn phys_range_to_offset(&self, phys: u64, len: usize) -> Result<usize> {
        let off = phys
            .checked_sub(self.phys)
            .and_then(|o| usize::try_from(o).ok())
            .ok_or(Error::InvalidAddress)?;
        self.check(off, len).map_err(|_| Error::InvalidAddress)?;
        Ok(off)
    }

    /// Physical address of `offset` (which may equal the size: one past the
    /// end).
    pub fn offset_to_phys(&self, offset: usize) -> Option<u64> {
        if offset <= self.size {
            Some(self.phys + offset as u64)
        } else {
            None
        }
    }

    /// Translates a virtual address to an offset into the region.
    pub fn virt_to_offset(&self, ptr: *const u8) -> Option<usize> {
        let off = (ptr as usize).checked_sub(self.virt.as_ptr() as usize)?;
        if off < self.size { Some(off) } else { None }
    }

    /// Virtual address of `offset`.
    pub fn offset_to_virt(&self, offset: usize) -> Option<*mut u8> {
        self.ptr(offset, 0).ok()
    }

    /// A region covering `len` bytes at `offset` of this one.
    pub fn subregion(&self, offset: usize, len: usize) -> Result<IoRegion> {
        let p = self.ptr(offset, len)?;
        Ok(IoRegion {
            // SAFETY: derived from a non-null base by an in-bounds offset.
            virt: unsafe { NonNull::new_unchecked(p) },
            phys: self.phys + offset as u64,
            size: len,
        })
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;

    #[test]
    fn bounds_are_enforced() {
        let io = IoRegion::alloc(64, 0x1000);
        assert_eq!(io.read_u32(60), Ok(0));
        assert_eq!(io.read_u32(57), Err(Error::Misaligned));
        assert_eq!(io.read_u32(64), Err(Error::OutOfBounds));
        assert_eq!(io.read_u64(usize::MAX - 2), Err(Error::OutOfBounds));
        let mut buf = [0u8; 8];
        assert_eq!(io.read_bytes(57, &mut buf), Err(Error::OutOfBounds));
        assert!(io.read_bytes(56, &mut buf).is_ok());
    }

    #[test]
    fn address_translation() {
        let io = IoRegion::alloc(0x100, 0x8000_0000);
        assert_eq!(io.phys_to_offset(0x8000_0010), Some(0x10));
        assert_eq!(io.phys_to_offset(0x7fff_ffff), None);
        assert_eq!(io.phys_to_offset(0x8000_0100), None);
        assert_eq!(io.phys_range_to_offset(0x8000_00f0, 0x10), Ok(0xf0));
        assert_eq!(
            io.phys_range_to_offset(0x8000_00f0, 0x11),
            Err(Error::InvalidAddress)
        );
        assert_eq!(io.offset_to_phys(0x100), Some(0x8000_0100));
        let sub = io.subregion(0x20, 0x10).unwrap();
        assert_eq!(sub.phys(), 0x8000_0020);
        sub.write_u32(0, 0xdead_beef).unwrap();
        assert_eq!(io.read_u32(0x20), Ok(0xdead_beef));
        assert_eq!(io.virt_to_offset(sub.as_ptr()), Some(0x20));
    }

    #[test]
    fn block_and_atomic_access() {
        let io = IoRegion::alloc(32, 0);
        io.write_bytes(4, b"hello").unwrap();
        let mut out = [0u8; 5];
        io.read_bytes(4, &mut out).unwrap();
        assert_eq!(&out, b"hello");
        io.store_u16(10, 7, Ordering::Release).unwrap();
        assert_eq!(io.load_u16(10, Ordering::Acquire), Ok(7));
        io.fill(0, 0xaa, 32).unwrap();
        assert_eq!(io.read_u8(31), Ok(0xaa));
    }
}
