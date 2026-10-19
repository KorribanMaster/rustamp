// SPDX-License-Identifier: BSD-3-Clause
//! RPU GIC (ARM PL390, GICv1) register encodings.
//!
//! The RPU has a GIC of its own, separate from the APU's, so the R5 owns its
//! distributor configuration.

/// Distributor control.
pub const GICD_CTLR: u32 = 0x000;
/// Set-enable, one bit per interrupt.
pub const GICD_ISENABLER: u32 = 0x100;
/// Priority, one byte per interrupt.
pub const GICD_IPRIORITYR: u32 = 0x400;
/// CPU targets, one byte per interrupt.
pub const GICD_ITARGETSR: u32 = 0x800;

/// CPU interface control.
pub const GICC_CTLR: u32 = 0x00;
/// Priority mask.
pub const GICC_PMR: u32 = 0x04;
/// Interrupt acknowledge.
pub const GICC_IAR: u32 = 0x0C;
/// End of interrupt.
pub const GICC_EOIR: u32 = 0x10;

/// Interrupt ID read from IAR when nothing is pending.
pub const SPURIOUS: u32 = 1023;

/// Offset of the `ISENABLER` word and the bit for interrupt `id`.
pub const fn isenabler(id: u32) -> (u32, u32) {
    (GICD_ISENABLER + 4 * (id / 32), 1 << (id % 32))
}

/// Offset of the priority byte of interrupt `id`.
pub const fn ipriorityr(id: u32) -> u32 {
    GICD_IPRIORITYR + id
}

/// Offset of the target byte of interrupt `id`.
pub const fn itargetsr(id: u32) -> u32 {
    GICD_ITARGETSR + id
}

/// Interrupt ID from an IAR value (bits 9:0).
pub const fn iar_id(iar: u32) -> u32 {
    iar & 0x3FF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipi_channel_1_encodings() {
        // IPI channel 1 is interrupt 65 on the RPU GIC.
        assert_eq!(isenabler(65), (0x108, 1 << 1));
        assert_eq!(ipriorityr(65), 0x441);
        assert_eq!(itargetsr(65), 0x841);
    }

    #[test]
    fn iar_id_masks_the_cpu_bits() {
        assert_eq!(iar_id(1 << 10 | 65), 65);
        assert_eq!(iar_id(SPURIOUS), SPURIOUS);
    }
}
