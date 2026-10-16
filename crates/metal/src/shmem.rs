// SPDX-License-Identifier: BSD-3-Clause
//! Shared-memory pools (`metal_shmem`, `rpmsg_virtio_shm_pool`).

use crate::{Error, IoRegion, Result, align_up};

/// A bump allocator handing out offsets into a shared region.
///
/// This mirrors OpenAMP's `rpmsg_virtio_shm_pool`: buffers are carved out
/// once during setup and never freed individually.
#[derive(Debug, Clone)]
pub struct ShmemPool {
    io: IoRegion,
    base: usize,
    size: usize,
    used: usize,
}

impl ShmemPool {
    /// A pool covering `size` bytes at `base` within `io`.
    pub fn new(io: IoRegion, base: usize, size: usize) -> Result<Self> {
        io.check(base, size)?;
        Ok(ShmemPool {
            io,
            base,
            size,
            used: 0,
        })
    }

    /// A pool covering all of `io`.
    pub fn whole(io: IoRegion) -> Self {
        let size = io.len();
        ShmemPool {
            io,
            base: 0,
            size,
            used: 0,
        }
    }

    /// The region the offsets refer to.
    pub fn io(&self) -> &IoRegion {
        &self.io
    }

    /// Allocates `size` bytes aligned to `align` (a power of two) and returns
    /// their offset within [`io`](Self::io).
    pub fn alloc(&mut self, size: usize, align: usize) -> Result<usize> {
        if !align.is_power_of_two() {
            return Err(Error::InvalidParam);
        }
        let start = align_up(self.base + self.used, align);
        let end = start.checked_add(size).ok_or(Error::NoMemory)?;
        if end > self.base + self.size {
            return Err(Error::NoMemory);
        }
        self.used = end - self.base;
        Ok(start)
    }

    /// Bytes still available (ignoring alignment padding).
    pub fn available(&self) -> usize {
        self.size - self.used
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;

    #[test]
    fn bump_allocation() {
        let io = IoRegion::alloc(0x1000, 0);
        let mut pool = ShmemPool::new(io, 0x10, 0x100).unwrap();
        assert_eq!(pool.alloc(4, 4), Ok(0x10));
        assert_eq!(pool.alloc(8, 0x40), Ok(0x40));
        assert_eq!(pool.alloc(0xc8, 8), Ok(0x48));
        assert_eq!(pool.available(), 0);
        assert_eq!(pool.alloc(1, 1), Err(Error::NoMemory));
        assert_eq!(pool.alloc(1, 3), Err(Error::InvalidParam));
    }
}
