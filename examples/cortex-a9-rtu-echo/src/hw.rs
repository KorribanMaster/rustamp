// SPDX-License-Identifier: BSD-3-Clause
//! GIC access for the RTU core: only its banked CPU-interface and SGI
//! registers, plus `GICD_SGIR` to raise SGIs on the Linux core.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cortex_a9_rtu_echo::board::{HOST_CPU, PARK_BASE, SGI_TO_HOST, SGI_TO_RTU};
use cortex_a9_rtu_echo::gic::*;
use cortex_a9_rtu_echo::park;

static GIC_BASE: AtomicU32 = AtomicU32::new(0);
/// Set by the IRQ handler when Linux kicked this core.
pub static KICKED: AtomicBool = AtomicBool::new(false);

fn reg(off: u32) -> *mut u32 {
    (GIC_BASE.load(Ordering::Relaxed) + off) as usize as *mut u32
}

/// Enables this core's GIC CPU interface and the wake-up SGI.
pub fn init(periphbase: u32) {
    GIC_BASE.store(periphbase, Ordering::Relaxed);
    let gicc = GICC_OFFSET;
    let gicd = GICD_OFFSET;
    // SAFETY: banked per-CPU GIC registers of this core; Linux on the other
    // core never sees these copies. The priority array is byte-accessible.
    unsafe {
        let prio = reg(gicd + GICD_IPRIORITYR) as *mut u8;
        write_volatile(prio.add(SGI_TO_RTU as usize), 0xA0);
        write_volatile(reg(gicd + GICD_ISENABLER), 1 << SGI_TO_RTU);
        write_volatile(reg(gicc + GICC_PMR), 0xF0);
        write_volatile(reg(gicc + GICC_CTLR), 1);
    }
}

/// Raises the host's SGI on the Linux core.
pub fn kick_host() {
    // SAFETY: GICD_SGIR is write-only; one write raises one SGI and does
    // not touch any state Linux configured.
    unsafe {
        core::arch::asm!("dsb", options(nostack, preserves_flags));
        write_volatile(reg(GICD_OFFSET + GICD_SGIR), sgir(HOST_CPU, SGI_TO_HOST));
    }
}

/// IRQ entry: acknowledge, note a kick, end the interrupt.
pub fn on_irq() {
    // SAFETY: banked CPU-interface registers of this core.
    unsafe {
        let iar = read_volatile(reg(GICC_OFFSET + GICC_IAR));
        let id = iar_id(iar);
        if id == SPURIOUS {
            return;
        }
        if id == SGI_TO_RTU {
            KICKED.store(true, Ordering::Release);
        }
        write_volatile(reg(GICC_OFFSET + GICC_EOIR), iar);
    }
}

/// Whether Linux asked this core to park (see `park.rs`).
pub fn stop_requested() -> bool {
    let p = (PARK_BASE + park::STOP_REQ_OFF) as usize as *const u32;
    // SAFETY: the park page is RTU memory, mapped uncached.
    unsafe { read_volatile(p) == park::STOP_REQ }
}
