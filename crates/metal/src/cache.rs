// SPDX-License-Identifier: BSD-3-Clause
//! Data-cache maintenance (`metal_cache_flush` / `metal_cache_invalidate`).
//!
//! Shared memory is best mapped non-cacheable (or the interconnect keeps it
//! coherent); then [`NoCache`] is correct. When message buffers are cacheable
//! on one side, the rpmsg layer flushes a buffer before handing it to the
//! peer and invalidates it after taking it back, through this trait.
//!
//! Ring structures themselves (descriptor table, avail and used rings) must
//! always be mapped non-cacheable or coherent: their indices are polled.

/// Cache maintenance by address range.
pub trait Cache: Sync {
    /// Writes dirty lines covering `addr..addr + len` back to memory.
    fn flush(&self, addr: *const u8, len: usize);
    /// Discards lines covering `addr..addr + len` so the next read fetches
    /// memory.
    fn invalidate(&self, addr: *const u8, len: usize);
}

/// No-op cache operations for coherent or uncached memory.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoCache;

impl Cache for NoCache {
    #[inline]
    fn flush(&self, _: *const u8, _: usize) {}
    #[inline]
    fn invalidate(&self, _: *const u8, _: usize) {}
}

#[inline]
#[allow(dead_code)]
fn lines(addr: *const u8, len: usize, line: usize) -> impl Iterator<Item = usize> {
    let start = crate::align_down(addr as usize, line);
    let end = (addr as usize).saturating_add(len);
    (start..end).step_by(line)
}

/// Cortex-M7 L1 data cache, maintained through the SCB by-address registers.
///
/// Invalidation discards whole 32-byte lines: buffers handed to
/// [`Cache::invalidate`] should be line-aligned and a whole number of lines,
/// or neighbouring data in the same line may be lost.
#[cfg(all(target_arch = "arm", feature = "cortex-m7"))]
#[derive(Debug, Default, Clone, Copy)]
pub struct CortexM7Cache;

#[cfg(all(target_arch = "arm", feature = "cortex-m7"))]
impl CortexM7Cache {
    const LINE: usize = 32;
    const DCIMVAC: *mut u32 = 0xE000_EF5C as *mut u32;
    const DCCMVAC: *mut u32 = 0xE000_EF68 as *mut u32;

    #[inline]
    fn barrier() {
        // SAFETY: DSB/ISB have no memory-safety effects.
        unsafe { core::arch::asm!("dsb sy", "isb sy", options(nostack, preserves_flags)) };
    }
}

#[cfg(all(target_arch = "arm", feature = "cortex-m7"))]
impl Cache for CortexM7Cache {
    fn flush(&self, addr: *const u8, len: usize) {
        Self::barrier();
        for line in lines(addr, len, Self::LINE) {
            // SAFETY: architecturally defined SCB register on every Cortex-M7.
            unsafe { Self::DCCMVAC.write_volatile(line as u32) };
        }
        Self::barrier();
    }

    fn invalidate(&self, addr: *const u8, len: usize) {
        Self::barrier();
        for line in lines(addr, len, Self::LINE) {
            // SAFETY: architecturally defined SCB register on every Cortex-M7.
            unsafe { Self::DCIMVAC.write_volatile(line as u32) };
        }
        Self::barrier();
    }
}

/// ARMv7-A L1/L2 data cache to the point of coherency (Cortex-A9 and
/// similar), using CP15 by-MVA operations with a 32-byte line.
#[cfg(all(target_arch = "arm", feature = "armv7a"))]
#[derive(Debug, Default, Clone, Copy)]
pub struct Armv7aCache;

#[cfg(all(target_arch = "arm", feature = "armv7a"))]
impl Cache for Armv7aCache {
    fn flush(&self, addr: *const u8, len: usize) {
        for line in lines(addr, len, 32) {
            // SAFETY: DCCMVAC (clean by MVA to PoC) only affects cache state.
            unsafe {
                core::arch::asm!("mcr p15, 0, {0}, c7, c10, 1", in(reg) line, options(nostack))
            };
        }
        // SAFETY: barrier only.
        unsafe { core::arch::asm!("dsb", options(nostack, preserves_flags)) };
    }

    fn invalidate(&self, addr: *const u8, len: usize) {
        for line in lines(addr, len, 32) {
            // SAFETY: DCIMVAC (invalidate by MVA to PoC) on a line the caller
            // owns; see the alignment note on `CortexM7Cache`.
            unsafe {
                core::arch::asm!("mcr p15, 0, {0}, c7, c6, 1", in(reg) line, options(nostack))
            };
        }
        // SAFETY: barrier only.
        unsafe { core::arch::asm!("dsb", options(nostack, preserves_flags)) };
    }
}

/// AArch64 data cache to the point of coherency, using `DC CVAC`/`DC IVAC`
/// (EL1 or higher) with the line size read from `CTR_EL0`.
#[cfg(target_arch = "aarch64")]
#[derive(Debug, Default, Clone, Copy)]
pub struct Aarch64Cache;

#[cfg(target_arch = "aarch64")]
impl Aarch64Cache {
    fn line() -> usize {
        let ctr: u64;
        // SAFETY: CTR_EL0 is readable at EL0 and above.
        unsafe { core::arch::asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
        4 << ((ctr >> 16) & 0xf)
    }
}

#[cfg(target_arch = "aarch64")]
impl Cache for Aarch64Cache {
    fn flush(&self, addr: *const u8, len: usize) {
        for line in lines(addr, len, Self::line()) {
            // SAFETY: clean by VA only affects cache state.
            unsafe { core::arch::asm!("dc cvac, {0}", in(reg) line, options(nostack)) };
        }
        // SAFETY: barrier only.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }

    fn invalidate(&self, addr: *const u8, len: usize) {
        for line in lines(addr, len, Self::line()) {
            // SAFETY: invalidate by VA on a range the caller owns (needs EL1).
            unsafe { core::arch::asm!("dc ivac, {0}", in(reg) line, options(nostack)) };
        }
        // SAFETY: barrier only.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }
}

#[cfg(test)]
mod tests {
    use std::vec;
    use std::vec::Vec;

    #[test]
    fn line_iteration_covers_range() {
        let v: Vec<usize> = super::lines(0x1004 as *const u8, 0x40, 32).collect();
        assert_eq!(v, vec![0x1000, 0x1020, 0x1040]);
        assert_eq!(super::lines(0x1000 as *const u8, 0, 32).count(), 0);
    }
}
