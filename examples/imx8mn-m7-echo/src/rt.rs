// SPDX-License-Identifier: BSD-3-Clause
//! A minimal Cortex-M7 runtime: vector table, reset, fault and interrupt
//! entry. Kept in-tree so the example has no dependencies beyond the
//! openamp crates; `cortex-m-rt` works just as well.

// The QEMU build polls the MU and never sleeps on an interrupt.
#![cfg_attr(feature = "qemu-imx8mp", allow(dead_code))]

use core::arch::{asm, naked_asm};

/// Number of device interrupts in the vector table (i.MX 8M Nano M7 has
/// fewer than 160).
const NUM_IRQS: usize = 160;

#[derive(Clone, Copy)]
#[repr(C)]
union Vector {
    handler: unsafe extern "C" fn(),
    reserved: usize,
}

#[repr(C)]
struct VectorTable {
    initial_sp: *const u32,
    reset: unsafe extern "C" fn() -> !,
    exceptions: [Vector; 14],
    irqs: [Vector; NUM_IRQS],
}

// SAFETY: the table is immutable and only read by the hardware.
unsafe impl Sync for VectorTable {}

unsafe extern "C" {
    static _stack_top: u32;
}

const fn h(f: unsafe extern "C" fn()) -> Vector {
    Vector { handler: f }
}

const R: Vector = Vector { reserved: 0 };

#[used]
#[unsafe(link_section = ".vector_table")]
static VECTORS: VectorTable = VectorTable {
    initial_sp: &raw const _stack_top,
    reset: Reset,
    exceptions: [
        h(fault), // NMI
        h(fault), // HardFault
        h(fault), // MemManage
        h(fault), // BusFault
        h(fault), // UsageFault
        R,
        R,
        R,
        R,
        h(default_handler), // SVCall
        h(default_handler), // DebugMonitor
        R,
        h(default_handler), // PendSV
        h(default_handler), // SysTick
    ],
    irqs: [h(irq_entry); NUM_IRQS],
};

/// Zeroes .bss, copies .data, enables the FPU, then calls `crate::main`.
/// Written in assembly because no Rust code may run before memory is set
/// up (and the hard-float ABI needs the FPU enabled).
#[unsafe(naked)]
#[unsafe(no_mangle)]
unsafe extern "C" fn Reset() -> ! {
    naked_asm!(
        "movw r0, :lower16:__sbss",
        "movt r0, :upper16:__sbss",
        "movw r1, :lower16:__ebss",
        "movt r1, :upper16:__ebss",
        "movs r2, #0",
        "0:",
        "cmp r0, r1",
        "bhs 1f",
        "str r2, [r0], #4",
        "b 0b",
        "1:",
        "movw r0, :lower16:__sdata",
        "movt r0, :upper16:__sdata",
        "movw r1, :lower16:__edata",
        "movt r1, :upper16:__edata",
        "movw r2, :lower16:__sidata",
        "movt r2, :upper16:__sidata",
        "2:",
        "cmp r0, r1",
        "bhs 3f",
        "ldr r3, [r2], #4",
        "str r3, [r0], #4",
        "b 2b",
        "3:",
        // CPACR: full access to CP10/CP11 (the FPU).
        "movw r0, :lower16:0xE000ED88",
        "movt r0, :upper16:0xE000ED88",
        "ldr r1, [r0]",
        "orr r1, r1, #(0xF << 20)",
        "str r1, [r0]",
        "dsb",
        "isb",
        "bl {main}",
        "4:",
        "b 4b",
        main = sym crate::main,
    )
}

unsafe extern "C" fn fault() {
    loop {
        // SAFETY: breakpoint for a debugger; harmless otherwise.
        unsafe { asm!("bkpt #0", options(nomem, nostack)) };
    }
}

unsafe extern "C" fn default_handler() {}

/// Common entry for all device interrupts: IPSR tells which one.
unsafe extern "C" fn irq_entry() {
    let ipsr: u32;
    // SAFETY: reading IPSR has no side effects.
    unsafe { asm!("mrs {}, IPSR", out(reg) ipsr, options(nomem, nostack, preserves_flags)) };
    crate::on_irq((ipsr & 0x1ff) as u16 - 16);
}

/// Waits for an interrupt with interrupts masked, unless `skip` says work
/// is already pending; a pending interrupt still ends the wait and is taken
/// once interrupts are unmasked again.
pub fn sleep_unless(skip: impl FnOnce() -> bool) {
    // SAFETY: masking/unmasking interrupts and WFI have no memory effects
    // beyond ordering.
    unsafe {
        asm!("cpsid i", options(nomem, nostack));
        if !skip() {
            asm!("dsb", "wfi", options(nomem, nostack));
        }
        asm!("cpsie i", options(nomem, nostack));
    }
}

/// Memory barriers.
pub fn barrier() {
    // SAFETY: barriers only.
    unsafe { asm!("dsb", "isb", options(nostack, preserves_flags)) };
}

/// Busy-waits roughly `cycles` core cycles.
pub fn delay_cycles(cycles: u32) {
    for _ in 0..cycles / 2 {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        // SAFETY: as in `fault`.
        unsafe { asm!("bkpt #1", options(nomem, nostack)) };
    }
}
