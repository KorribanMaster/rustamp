// SPDX-License-Identifier: BSD-3-Clause
//! Memory map and per-SoC presets.
//!
//! The memory map is the same on every board: the RTU owns the top 32 MiB of
//! the first GiB of DDR, which the Linux device tree reserves (`no-map`).
//!
//! | region            | address      | size    | RTU mapping              |
//! |-------------------|--------------|---------|--------------------------|
//! | firmware image    | `0x3E000000` | 15 MiB  | inner WB, outer non-cacheable |
//! | resource table    | `0x3EF00000` | 1 MiB   | non-cacheable (shared)   |
//! | park page         | `0x3EFFF000` | 4 KiB   | (in the section above)   |
//! | vring 0           | `0x3F000000` | 32 KiB  | non-cacheable (shared)   |
//! | vring 1           | `0x3F008000` | 32 KiB  | non-cacheable (shared)   |
//! | rpmsg buffers     | `0x3F010000` | 1 MiB   | non-cacheable (shared)   |
//!
//! Firmware memory is outer non-cacheable so the RTU never allocates lines
//! in the PL310 L2, which Linux owns and maintains (it may clean or
//! invalidate it by way at any time). Shared memory is non-cacheable on both
//! sides, so no cache maintenance is needed for rpmsg. `rtu.x` must agree
//! with the constants here; a test checks that.
//!
//! Everything else the RTU needs — the GIC and SCU base — is read from the
//! CPU's CBAR register at run time, so the firmware is not tied to one SoC.
//! A preset only adds the debug UART and the device sections to map.

/// Start of the RTU's reserved DDR.
pub const RTU_BASE: u32 = 0x3E00_0000;
/// Size of the RTU's reserved DDR (firmware, resource table, shared memory).
pub const RTU_SIZE: u32 = 0x0200_0000;
/// Firmware image (code, data, stacks, MMU table).
pub const FW_BASE: u32 = RTU_BASE;
/// Size of the firmware image region.
pub const FW_SIZE: u32 = 0x00F0_0000;
/// Resource table section (shared with Linux, uncached).
pub const RSC_BASE: u32 = FW_BASE + FW_SIZE;
/// Size of the resource-table section, park page included.
pub const RSC_SIZE: u32 = 0x0010_0000;
/// The page the RTU parks in between stop and the next start; see [`crate::park`].
pub const PARK_BASE: u32 = RSC_BASE + RSC_SIZE - 0x1000;
/// Shared memory for vrings and rpmsg buffers.
pub const SHM_BASE: u32 = RSC_BASE + RSC_SIZE;
/// Size of the shared memory region.
pub const SHM_SIZE: u32 = RTU_BASE + RTU_SIZE - SHM_BASE;
/// vring 0 (RTU → Linux).
pub const VRING0: u32 = SHM_BASE;
/// vring 1 (Linux → RTU).
pub const VRING1: u32 = SHM_BASE + 0x8000;
/// vring alignment, as Linux remoteproc expects.
pub const VRING_ALIGN: u32 = 0x1000;
/// Entries per vring.
pub const VRING_NUM: u32 = 256;
/// rpmsg buffer pool (Linux allocates it from its `vdev0buffer` region).
pub const BUF_BASE: u32 = SHM_BASE + 0x1_0000;
/// Size of the rpmsg buffer pool.
pub const BUF_SIZE: u32 = 0x10_0000;

/// SGI Linux sends to wake the RTU (and to kick it).
pub const SGI_TO_RTU: u32 = 15;
/// SGI the RTU sends to Linux.
pub const SGI_TO_HOST: u32 = 14;
/// The CPU Linux runs on.
pub const HOST_CPU: u32 = 0;

/// Debug UART flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UartKind {
    /// Cadence UART (Zynq-7000 `xuartps`).
    Cadence,
    /// 16550-compatible with 32-bit registers (Arria 10 `snps,dw-apb-uart`).
    Ns16550,
}

/// A SoC preset.
#[derive(Debug, Clone, Copy)]
pub struct Board {
    /// Human-readable name for the boot log.
    pub name: &'static str,
    /// Debug UART type.
    pub uart: UartKind,
    /// Debug UART base address. Linux must not use this UART, and must keep
    /// its clock running (`clk_ignore_unused` or a DT clock consumer).
    pub uart_base: u32,
    /// Further 1 MiB device sections to map (besides PERIPHBASE and the UART).
    pub device_sections: &'static [u32],
}

/// Zynq-7000: UART0 for the RTU (Linux uses UART1 on the ZC702).
pub const ZYNQ7000: Board = Board {
    name: "zynq-7000",
    uart: UartKind::Cadence,
    uart_base: 0xE000_0000,
    device_sections: &[],
};

/// Arria 10 SoC: UART0 for the RTU (Linux uses UART1 on the SoCDK).
pub const ARRIA10: Board = Board {
    name: "arria10",
    uart: UartKind::Ns16550,
    uart_base: 0xFFC0_2000,
    device_sections: &[],
};

/// The board this firmware is built for.
#[cfg(not(feature = "arria10"))]
pub const BOARD: Board = ZYNQ7000;
/// The board this firmware is built for.
#[cfg(feature = "arria10")]
pub const BOARD: Board = ARRIA10;

// The regions fit inside each other (checked at build time).
const _: () = {
    assert!(VRING0 + 0x8000 <= VRING1);
    assert!(VRING1 + 0x8000 <= BUF_BASE);
    assert!(BUF_BASE + BUF_SIZE <= SHM_BASE + SHM_SIZE);
    assert!(PARK_BASE >= RSC_BASE && PARK_BASE + 0x1000 == SHM_BASE);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `NAME : ORIGIN = 0x..., LENGTH = 0x...` from `rtu.x`.
    fn region(name: &str) -> (u32, u32) {
        let x = include_str!("../rtu.x");
        let line = x
            .lines()
            .find(|l| l.trim_start().starts_with(name) && l.contains("ORIGIN"))
            .unwrap_or_else(|| panic!("{name} not in rtu.x"));
        let num = |key: &str| {
            let s = line.split(key).nth(1).unwrap();
            let s = s.trim_start_matches([' ', '=']).trim_start();
            let hex = s.trim_start_matches("0x");
            let end = hex
                .find(|c: char| !c.is_ascii_hexdigit())
                .unwrap_or(hex.len());
            u32::from_str_radix(&hex[..end], 16).unwrap()
        };
        (num("ORIGIN"), num("LENGTH"))
    }

    #[test]
    fn linker_script_matches_memory_map() {
        assert_eq!(region("FW"), (FW_BASE, FW_SIZE));
        assert_eq!(region("RSC"), (RSC_BASE, RSC_SIZE - 0x1000));
    }

    #[test]
    fn regions_are_section_aligned_and_inside_the_carve_out() {
        for a in [RTU_BASE, FW_BASE, RSC_BASE, SHM_BASE] {
            assert_eq!(a % 0x10_0000, 0, "{a:#x} not on a 1 MiB section");
        }
        assert_eq!(SHM_BASE + SHM_SIZE, RTU_BASE + RTU_SIZE);
    }

    #[test]
    fn sgis_are_outside_linux_ipis() {
        // Linux allocates SGIs 0..7 for its own IPIs (gic_smp_init).
        assert!((8..16).contains(&SGI_TO_RTU));
        assert!((8..16).contains(&SGI_TO_HOST));
        assert_ne!(SGI_TO_RTU, SGI_TO_HOST);
    }
}
