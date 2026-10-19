// SPDX-License-Identifier: BSD-3-Clause
//! Memory map and board constants (R5_0, split mode).
//!
//! The DDR addresses follow the reserved-memory nodes of mainline Linux's
//! `zynqmp.dtsi` (`rproc_0_fw_image`) and of upstream OpenAMP's
//! `zynqmp-openamp.dtso` (vrings and buffers), which our overlay adds.

/// `rproc_0_fw_image`: resource table and trace buffer.
pub const FW_DDR_BASE: u32 = 0x3ED0_0000;
/// Size of `rproc_0_fw_image`.
pub const FW_DDR_SIZE: u32 = 0x4_0000;
/// The trace buffer (`memory.x` places `.rsc_trace` here).
pub const TRACE_DA: u32 = 0x3ED0_1000;
/// Trace buffer size.
pub const TRACE_LEN: u32 = 0x1000;

/// Shared memory: `vdev0vring0`, `vdev0vring1`, `vdev0buffer`.
pub const SHM_BASE: u32 = 0x3ED4_0000;
/// 16 KiB per vring plus the 1 MiB buffer pool.
pub const SHM_SIZE: u32 = 0x8000 + 0x10_0000;
/// Ring entries per vring (upstream: 256).
pub const VRING_NUM: u32 = 256;
/// Used-ring alignment.
pub const VRING_ALIGN: u32 = 0x1000;

/// The 4 MiB MPU region marked non-cacheable: everything Linux and the R5
/// share (table, trace, vrings, buffers).
pub const SHARED_MPU_BASE: u32 = 0x3EC0_0000;
/// Size of that region.
pub const SHARED_MPU_SIZE: u32 = 0x40_0000;

/// IPI channel 1 (the RPU0 agent).
pub const IPI_BASE: u32 = 0xFF31_0000;
/// Its interrupt on the RPU GIC.
pub const IPI_IRQ: u32 = 65;
/// The host's IPI agent mask. Mainline `zynqmp.dtsi` puts the APU on
/// channel 0 (`xlnx,ipi-id = <0>`). Older Xilinx device trees use channel 7:
/// `0x0100_0000`.
pub const HOST_IPI_MASK: u32 = 1 << 0;

/// RPU GIC (PL390): distributor and CPU interface.
pub const GICD_BASE: u32 = 0xF900_0000;
/// CPU interface.
pub const GICC_BASE: u32 = 0xF900_1000;

/// Debug UART (Cadence). Linux's console is UART0 on the ZCU102, so the R5
/// writes to UART1.
#[cfg(feature = "zcu102")]
pub const UART_BASE: u32 = 0xFF01_0000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_memory_is_inside_the_uncached_region() {
        let end = |b: u32, s: u32| b as u64 + s as u64;
        assert_eq!(SHARED_MPU_BASE % SHARED_MPU_SIZE, 0, "MPU alignment");
        for (b, s) in [(FW_DDR_BASE, FW_DDR_SIZE), (SHM_BASE, SHM_SIZE)] {
            assert!(b >= SHARED_MPU_BASE);
            assert!(end(b, s) <= end(SHARED_MPU_BASE, SHARED_MPU_SIZE));
        }
    }

    #[test]
    fn trace_buffer_is_in_the_firmware_carveout() {
        assert!(TRACE_DA >= FW_DDR_BASE + 0x1000, "after the resource table");
        assert!(TRACE_DA + TRACE_LEN <= FW_DDR_BASE + FW_DDR_SIZE);
    }

    #[test]
    fn vrings_and_buffers_fit_the_reserved_memory() {
        // vdev0vring0/1 are 16 KiB each in the device tree.
        let vring = 16 * VRING_NUM + 2 * (3 + VRING_NUM);
        let used = 2 * 3 + 8 * VRING_NUM;
        let size = vring.next_multiple_of(VRING_ALIGN) + used;
        assert!(size <= 0x4000, "vring needs {size:#x} bytes");
    }
}
