// SPDX-License-Identifier: BSD-3-Clause
//! The R5's side of the hardware: the IPI channel to the host, the RPU GIC
//! and the MPU.

use core::ptr::{read_volatile, write_volatile};
#[cfg(not(feature = "qemu"))]
use core::sync::atomic::{AtomicBool, Ordering};

use aarch32_cpu::pmsav7::{AccessPerms, Config, MemAttr, Mpu, Region, RegionSize};
use zynqmp_r5_echo::board::*;
#[cfg(not(feature = "qemu"))]
use zynqmp_r5_echo::gic;
use zynqmp_r5_echo::ipi;

/// Set by the IPI interrupt, cleared by the main loop.
#[cfg(not(feature = "qemu"))]
pub static KICKED: AtomicBool = AtomicBool::new(false);

fn rd(base: u32, off: u32) -> u32 {
    // SAFETY: fixed SoC registers of the RPU (IPI channel 1, RPU GIC), owned
    // by this core.
    unsafe { read_volatile((base + off) as usize as *const u32) }
}

fn wr(base: u32, off: u32, v: u32) {
    // SAFETY: as in `rd`.
    unsafe { write_volatile((base + off) as usize as *mut u32, v) }
}

#[cfg(not(feature = "qemu"))]
fn wr8(base: u32, off: u32, v: u8) {
    // SAFETY: as in `rd`; the GIC's priority and target registers are byte
    // accessible.
    unsafe { write_volatile((base + off) as usize as *mut u8, v) }
}

/// Routes IPI channel 1 to this core and enables the host as a source.
#[cfg(not(feature = "qemu"))]
pub fn init_irq() {
    wr8(GICD_BASE, gic::ipriorityr(IPI_IRQ), 0xA0);
    wr8(GICD_BASE, gic::itargetsr(IPI_IRQ), 0x01); // R5_0
    let (off, bit) = gic::isenabler(IPI_IRQ);
    wr(GICD_BASE, off, bit);
    wr(GICD_BASE, gic::GICD_CTLR, 1);
    wr(GICC_BASE, gic::GICC_PMR, 0xF0);
    wr(GICC_BASE, gic::GICC_CTLR, 1);
    // Drop anything left over from a previous run, then listen to the host.
    wr(IPI_BASE, ipi::ISR, HOST_IPI_MASK);
    wr(IPI_BASE, ipi::IER, HOST_IPI_MASK);
}

/// The QEMU model has no RPU GIC: only clear stale kicks; the main loop
/// polls `ISR` with [`poll_kick`].
#[cfg(feature = "qemu")]
pub fn init_irq() {
    wr(IPI_BASE, ipi::ISR, HOST_IPI_MASK);
}

/// Acknowledges a pending host kick; returns whether there was one. The
/// host's mailbox waits for this acknowledgement before kicking again.
#[cfg(feature = "qemu")]
pub fn poll_kick() -> bool {
    let pending = ipi::pending_from(rd(IPI_BASE, ipi::ISR), HOST_IPI_MASK);
    if pending != 0 {
        wr(IPI_BASE, ipi::ISR, pending);
    }
    pending != 0
}

/// Kicks the host (the virtio notification; the host services all vrings).
pub fn kick_host() {
    wr(IPI_BASE, ipi::TRIG, HOST_IPI_MASK);
}

/// Every IRQ lands here: acknowledge a host kick and wake the main loop.
#[cfg(not(feature = "qemu"))]
#[aarch32_rt::irq]
fn irq() {
    let iar = rd(GICC_BASE, gic::GICC_IAR);
    if gic::iar_id(iar) == IPI_IRQ {
        let pending = ipi::pending_from(rd(IPI_BASE, ipi::ISR), HOST_IPI_MASK);
        // Acknowledging clears the host's OBS bit, so it may kick again.
        wr(IPI_BASE, ipi::ISR, pending);
        KICKED.store(true, Ordering::Release);
    }
    if gic::iar_id(iar) != gic::SPURIOUS {
        wr(GICC_BASE, gic::GICC_EOIR, iar);
    }
}

/// Memory attributes: everything shared with Linux is non-cacheable, the
/// peripherals are device memory, the background map covers the rest. The
/// caches stay off (their reset state), so this only matters if someone
/// enables them; it then keeps rpmsg coherent without cache maintenance.
pub fn init_mpu() {
    static REGIONS: [Region; 2] = [
        Region {
            base: SHARED_MPU_BASE as usize as *mut u8,
            size: RegionSize::_4M,
            subregion_mask: 0,
            enabled: true,
            no_exec: true,
            mem_attr: MemAttr::NonCacheable { shareable: true },
            access_perms: AccessPerms::ReadWrite,
        },
        Region {
            base: 0xF800_0000usize as *mut u8,
            size: RegionSize::_128M,
            subregion_mask: 0,
            enabled: true,
            no_exec: true,
            mem_attr: MemAttr::Device { shareable: true },
            access_perms: AccessPerms::ReadWrite,
        },
    ];
    // SAFETY: only this function touches the MPU, before anything relies on
    // memory attributes.
    let mut mpu = unsafe { Mpu::new() };
    let cfg = Config {
        background_config: true,
        iregions: &[],
        dregions: &REGIONS,
    };
    if mpu.configure(&cfg).is_ok() {
        aarch32_cpu::asm::dsb();
        mpu.enable();
        aarch32_cpu::asm::isb();
    }
}
