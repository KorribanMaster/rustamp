// SPDX-License-Identifier: BSD-3-Clause
//! Cortex-A9 MPCore GIC (PL390-compatible, GICv1) register encodings.
//!
//! Every A9 cluster puts its private peripherals at PERIPHBASE, which the
//! CPU reports in CBAR: the CPU interface at +0x100 and the distributor at
//! +0x1000. The RTU only uses per-CPU (banked) registers and the
//! write-only `GICD_SGIR`, so it never races Linux on distributor state.

/// CPU interface offset from PERIPHBASE.
pub const GICC_OFFSET: u32 = 0x100;
/// Distributor offset from PERIPHBASE.
pub const GICD_OFFSET: u32 = 0x1000;

/// CPU interface control.
pub const GICC_CTLR: u32 = 0x00;
/// Priority mask.
pub const GICC_PMR: u32 = 0x04;
/// Interrupt acknowledge.
pub const GICC_IAR: u32 = 0x0C;
/// End of interrupt.
pub const GICC_EOIR: u32 = 0x10;

/// Set-enable registers (word 0 covers SGIs/PPIs and is banked per CPU).
pub const GICD_ISENABLER: u32 = 0x100;
/// Priority bytes (bytes 0..31 are banked per CPU).
pub const GICD_IPRIORITYR: u32 = 0x400;
/// Software-generated interrupt register.
pub const GICD_SGIR: u32 = 0xF00;

/// Interrupt ID read from IAR when nothing is pending.
pub const SPURIOUS: u32 = 1023;

/// `GICD_SGIR` value sending SGI `id` to CPU `cpu` (target-list filter 0).
pub const fn sgir(cpu: u32, id: u32) -> u32 {
    (1 << (16 + cpu)) | (id & 0xF)
}

/// Interrupt ID from an IAR value (bits 9:0; bits 12:10 are the source CPU
/// of an SGI and must be written back unchanged to EOIR).
pub const fn iar_id(iar: u32) -> u32 {
    iar & 0x3FF
}

/// PERIPHBASE from a CBAR value (bits 31:13 on the A9).
pub const fn periphbase(cbar: u32) -> u32 {
    cbar & 0xFFFF_E000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgir_targets_one_cpu() {
        assert_eq!(sgir(0, 14), 0x0001_000E);
        assert_eq!(sgir(1, 15), 0x0002_000F);
    }

    #[test]
    fn iar_keeps_source_cpu_out_of_the_id() {
        // SGI 15 from CPU 1.
        assert_eq!(iar_id(1 << 10 | 15), 15);
        assert_eq!(iar_id(SPURIOUS), SPURIOUS);
    }

    #[test]
    fn periphbase_of_known_socs() {
        assert_eq!(periphbase(0xF8F0_0000), 0xF8F0_0000); // Zynq-7000
        assert_eq!(periphbase(0xFFFF_C000), 0xFFFF_C000); // Arria 10
        assert_eq!(periphbase(0x1E00_0001), 0x1E00_0000); // Versatile Express
    }
}
