// SPDX-License-Identifier: BSD-3-Clause
//! A minimal Cortex-A9 runtime for the RTU core: vectors, reset, MMU and L1
//! cache bring-up, IRQ entry, and the jump into the park stub.
//!
//! Linux owns everything the two cores share (SCU, PL310 L2, the GIC
//! distributor configuration), so unlike a single-OS boot this code touches
//! none of it: only this core's CP15 state, its L1 caches and its MMU.
//! `ACTLR.SMP` stays clear, so this core does not join Linux's coherency
//! domain; shared memory is uncached instead.

use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;

use cortex_a9_rtu_echo::board::{self, BOARD};
use cortex_a9_rtu_echo::{mmu, park};

// Invalidates the whole L1 data cache by set/way, discarding dirty lines,
// using only r0-r7 (no memory accesses, so it is safe with caches off).
global_asm!(
    ".macro dcache_inv_all",
    "    mov r0, #0",
    "    mcr p15, 2, r0, c0, c0, 0", // CSSELR: L1 data cache
    "    isb",
    "    mrc p15, 1, r0, c0, c0, 0", // CCSIDR
    "    and r1, r0, #7",
    "    add r1, r1, #4", // log2(line bytes)
    "    ldr r3, =0x3ff",
    "    and r2, r3, r0, lsr #3", // ways - 1
    "    ldr r3, =0x7fff",
    "    and r3, r3, r0, lsr #13", // sets - 1
    "    clz r4, r2",              // way shift
    "    mov r6, r3",
    "1:  mov r5, r2",
    "2:  lsl r7, r5, r4",
    "    orr r7, r7, r6, lsl r1",
    "    mcr p15, 0, r7, c7, c6, 2", // DCISW
    "    subs r5, r5, #1",
    "    bge 2b",
    "    subs r6, r6, #1",
    "    bge 1b",
    "    dsb",
    ".endm",
);

// Vectors at the ELF entry (FW_BASE), reset code, exception trampolines.
global_asm!(
    ".section .vectors, \"ax\", %progbits",
    ".arm",
    ".global _start",
    "_start:",
    "    b _boot",
    "    b _und",
    "    b _svc",
    "    b _pabt",
    "    b _dabt",
    "    nop",
    "    b _irq",
    "    b _fiq",
    //
    ".section .text.boot, \"ax\", %progbits",
    ".arm",
    "_boot:",
    "    cpsid if",
    // MMU, caches, branch prediction and high vectors off (they are after a
    // reset; after a park they were switched off by `rtu_park_jump`).
    "    mrc p15, 0, r0, c1, c0, 0",
    "    bic r0, r0, #0x1",
    "    bic r0, r0, #0x4",
    "    bic r0, r0, #0x800",
    "    bic r0, r0, #0x1000",
    "    bic r0, r0, #0x2000",
    "    mcr p15, 0, r0, c1, c0, 0",
    "    isb",
    "    ldr r0, =_start",
    "    mcr p15, 0, r0, c12, c0, 0",   // VBAR
    "    dcache_inv_all",
    "    mov r0, #0",
    "    mcr p15, 0, r0, c7, c5, 0",    // ICIALLU
    "    mcr p15, 0, r0, c7, c5, 6",    // BPIALL
    "    mcr p15, 0, r0, c8, c7, 0",    // TLBIALL
    "    dsb",
    "    isb",
    "    cps #0x1b",
    "    ldr sp, =__und_stack",
    "    cps #0x17",
    "    ldr sp, =__abt_stack",
    "    cps #0x13",
    "    ldr sp, =__svc_stack",
    "    cps #0x12",
    "    ldr sp, =__irq_stack",
    "    cps #0x1f",
    "    ldr sp, =__sys_stack",
    "    bl rtu_start",
    "    b .",
    //
    ".section .text, \"ax\", %progbits",
    ".arm",
    "_irq:",
    "    sub lr, lr, #4",
    "    push {{r0-r3, r12, lr}}",
    "    bl rtu_irq",
    "    pop {{r0-r3, r12, lr}}",
    "    movs pc, lr",
    "_und:",
    "    mov r0, #1",
    "    mov r1, lr",
    "    b rtu_fault",
    "_svc:",
    "    mov r0, #2",
    "    mov r1, lr",
    "    b rtu_fault",
    "_pabt:",
    "    mov r0, #3",
    "    mov r1, lr",
    "    b rtu_fault",
    "_dabt:",
    "    mov r0, #4",
    "    mov r1, lr",
    "    b rtu_fault",
    "_fiq:",
    "    b _fiq",
    //
    // Turns the MMU and caches off, discards L1 (nothing of this image may
    // be written back over the next one) and jumps to the park stub in r0.
    ".global rtu_park_jump",
    "rtu_park_jump:",
    "    cpsid if",
    "    mov r8, r0",
    "    mrc p15, 0, r0, c1, c0, 0",
    "    bic r0, r0, #0x1",
    "    bic r0, r0, #0x4",
    "    bic r0, r0, #0x800",
    "    bic r0, r0, #0x1000",
    "    mcr p15, 0, r0, c1, c0, 0",
    "    isb",
    "    dcache_inv_all",
    "    mov r0, #0",
    "    mcr p15, 0, r0, c7, c5, 0",
    "    mcr p15, 0, r0, c7, c5, 6",
    "    mcr p15, 0, r0, c8, c7, 0",
    "    dsb",
    "    isb",
    "    bx r8",
    //
    // The park stub, copied to PARK_BASE. Position-independent: only
    // PC-relative literal loads. See `park.rs` for the protocol.
    ".global rtu_park_stub_start",
    ".global rtu_park_stub_end",
    "rtu_park_stub_start:",
    "    ldr r4, 9f",                    // park page
    "    mrc p15, 4, r5, c15, c0, 0",    // CBAR
    "    ldr r6, 8f",
    "    and r5, r5, r6",
    "    add r5, r5, #0x100",            // GIC CPU interface
    "    ldr r0, 7f",
    "    str r0, [r4, #{magic_off}]",
    "    mov r0, #0",
    "    str r0, [r4, #{stop_off}]",
    "    dsb",
    "1:  wfi",
    "    ldr r0, [r5, #0x0c]",           // IAR: acknowledge the wake-up SGI
    "    ldr r6, 6f",
    "    and r1, r0, r6",
    "    ldr r6, 5f",
    "    cmp r1, r6",
    "    strne r0, [r5, #0x10]",         // EOIR
    "    ldr r0, [r4, #{release_off}]",
    "    cmp r0, #0",
    "    beq 1b",
    "    mov r1, #0",
    "    str r1, [r4, #{release_off}]",
    "    str r1, [r4, #{magic_off}]",
    "    dsb",
    "    bx r0",
    "    .align 2",
    "5:  .word 1023",
    "6:  .word 0x3ff",
    "7:  .word {magic}",
    "8:  .word 0xffffe000",
    "9:  .word {park_base}",
    "rtu_park_stub_end:",
    magic_off = const park::MAGIC_OFF,
    stop_off = const park::STOP_REQ_OFF,
    release_off = const park::RELEASE_OFF,
    magic = const park::MAGIC,
    park_base = const board::PARK_BASE,
);

unsafe extern "C" {
    static mut __sbss: u32;
    static mut __ebss: u32;
    static rtu_park_stub_start: u8;
    static rtu_park_stub_end: u8;
    fn rtu_park_jump(stub: u32) -> !;
}

#[repr(C, align(16384))]
struct L1Table(UnsafeCell<[u32; 4096]>);

// SAFETY: written once by `rtu_start` before the MMU is on, then only read
// by the table walker.
unsafe impl Sync for L1Table {}

#[unsafe(link_section = ".data.mmu_table")]
static TABLE: L1Table = L1Table(UnsafeCell::new(mmu::table(&BOARD)));

/// PERIPHBASE of this A9 cluster (SCU, GIC, private timers), from CBAR.
pub fn periphbase() -> u32 {
    let cbar: u32;
    // SAFETY: reads a CP15 ID register.
    unsafe { asm!("mrc p15, 4, {0}, c15, c0, 0", out(reg) cbar, options(nomem, nostack)) };
    cortex_a9_rtu_echo::gic::periphbase(cbar)
}

/// Reset entry after the asm prologue: MMU off, caches off, stacks set.
#[unsafe(no_mangle)]
extern "C" fn rtu_start() -> ! {
    // SAFETY: linker-provided bounds of .bss; nothing uses .bss yet. Linux's
    // ELF loader zeroes it too, but a restart from the park stub does not
    // go through the loader for .bss contents left by the previous run.
    unsafe {
        let mut p = &raw mut __sbss;
        while p < &raw mut __ebss {
            p.write_volatile(0);
            p = p.add(1);
        }
    }

    let pb = periphbase();
    // SAFETY: the MMU is off, so nothing walks the table while it changes.
    unsafe { (*TABLE.0.get())[(pb >> 20) as usize] = mmu::section(pb, mmu::DEVICE) };

    // SAFETY: the table identity-maps this code, its data and stacks, so
    // execution continues at the same addresses once the MMU is on.
    unsafe {
        asm!(
            "mcr p15, 0, {z}, c2, c0, 2",    // TTBCR: TTBR0 only, 4 GiB
            "mcr p15, 0, {tbl}, c2, c0, 0",  // TTBR0 (uncached walks)
            "mov {t}, #1",
            "mcr p15, 0, {t}, c3, c0, 0",    // DACR: domain 0 = client
            "mcr p15, 0, {z}, c8, c7, 0",    // TLBIALL
            "dsb",
            "isb",
            "mrc p15, 0, {t}, c1, c0, 0",
            "orr {t}, {t}, #0x1",            // M
            "orr {t}, {t}, #0x4",            // C
            "orr {t}, {t}, #0x800",          // Z
            "orr {t}, {t}, #0x1000",         // I
            "mcr p15, 0, {t}, c1, c0, 0",
            "isb",
            z = in(reg) 0u32,
            tbl = in(reg) TABLE.0.get() as u32,
            t = out(reg) _,
            options(nostack),
        );
    }
    crate::main()
}

/// Copies the park stub into the park page and jumps there. Never returns;
/// Linux restarts the core at a new entry.
pub fn park() -> ! {
    // SAFETY: masks interrupts on this core only.
    unsafe { asm!("cpsid if", options(nomem, nostack)) };
    let src = &raw const rtu_park_stub_start;
    let len = &raw const rtu_park_stub_end as usize - src as usize;
    assert!(len as u32 <= park::STUB_MAX);
    let dst = board::PARK_BASE as usize as *mut u32;
    // SAFETY: the park page is RTU memory mapped executable and uncached;
    // the stub is position-independent and word-aligned.
    unsafe {
        for i in 0..len.div_ceil(4) {
            dst.add(i).write_volatile((src as *const u32).add(i).read());
        }
        asm!("dsb", options(nostack, preserves_flags));
        rtu_park_jump(board::PARK_BASE)
    }
}

/// Runs `ready` with interrupts masked and sleeps (`wfi`) if it returns
/// false; the pending interrupt that ends the sleep is taken right after.
pub fn sleep_unless(ready: impl Fn() -> bool) {
    // SAFETY: masking and unmasking IRQs on this core only.
    unsafe {
        asm!("cpsid i", options(nomem, nostack));
        if !ready() {
            asm!("dsb", "wfi", options(nomem, nostack));
        }
        asm!("cpsie i", options(nomem, nostack));
    }
}

/// Unmasks IRQs on this core.
pub fn enable_irqs() {
    // SAFETY: the vector table and IRQ stack are set up.
    unsafe { asm!("cpsie i", options(nomem, nostack)) };
}

#[unsafe(no_mangle)]
extern "C" fn rtu_irq() {
    crate::hw::on_irq();
}

#[unsafe(no_mangle)]
extern "C" fn rtu_fault(kind: u32, lr: u32) -> ! {
    let what = match kind {
        1 => "undefined instruction",
        2 => "svc",
        3 => "prefetch abort",
        4 => "data abort",
        _ => "exception",
    };
    let (dfsr, dfar): (u32, u32);
    // SAFETY: reads CP15 fault status registers.
    unsafe {
        asm!("mrc p15, 0, {0}, c5, c0, 0", out(reg) dfsr, options(nomem, nostack));
        asm!("mrc p15, 0, {0}, c6, c0, 0", out(reg) dfar, options(nomem, nostack));
    }
    crate::log!(
        "openamp-rs a9: {} at lr={:#x} dfsr={:#x} dfar={:#x}",
        what,
        lr,
        dfsr,
        dfar
    );
    loop {
        // SAFETY: halts this core.
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
