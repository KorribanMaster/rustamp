// SPDX-License-Identifier: BSD-3-Clause
//! The RTU's L1 translation table: 4096 identity-mapped 1 MiB sections
//! (ARMv7-A short descriptors). Anything not listed faults, so a stray
//! access to Linux's memory is caught instead of corrupting it.

use crate::board::{self, Board};

/// Section descriptor type bits.
const SECTION: u32 = 0b10;
/// AP[1:0] = 0b11: read/write at any privilege.
const AP_RW: u32 = 0b11 << 10;
/// Execute-never.
const XN: u32 = 1 << 4;
/// Shareable.
const S: u32 = 1 << 16;
const fn tex(t: u32) -> u32 {
    t << 12
}
const C: u32 = 1 << 3;
const B: u32 = 1 << 2;

/// RTU-private memory: inner write-back (no write-allocate), outer
/// non-cacheable (TEX = 0b1_00, C = 1, B = 1), so the shared L2 never holds
/// RTU lines.
pub const PRIVATE: u32 = SECTION | AP_RW | tex(0b100) | C | B;
/// Memory shared with Linux: normal, non-cacheable, shareable, execute-never.
pub const SHARED: u32 = SECTION | AP_RW | tex(0b001) | S | XN;
/// Shared memory the RTU executes from (the park page lives here).
pub const SHARED_EXEC: u32 = SECTION | AP_RW | tex(0b001) | S;
/// Device memory: shareable device, execute-never.
pub const DEVICE: u32 = SECTION | AP_RW | B | XN;

/// Descriptor for the section containing `addr` with attributes `attr`.
pub const fn section(addr: u32, attr: u32) -> u32 {
    (addr & 0xFFF0_0000) | attr
}

/// Builds the table for `b`. The PERIPHBASE section is added at run time
/// from CBAR (see `rt.rs`), so the table does not depend on the SoC's
/// private-peripheral address.
pub const fn table(b: &Board) -> [u32; 4096] {
    let mut t = [0u32; 4096];
    let mut a = board::FW_BASE;
    while a < board::FW_BASE + board::FW_SIZE {
        t[(a >> 20) as usize] = section(a, PRIVATE);
        a += 0x10_0000;
    }
    t[(board::RSC_BASE >> 20) as usize] = section(board::RSC_BASE, SHARED_EXEC);
    let mut a = board::SHM_BASE;
    while a < board::SHM_BASE + board::SHM_SIZE {
        t[(a >> 20) as usize] = section(a, SHARED);
        a += 0x10_0000;
    }
    t[(b.uart_base >> 20) as usize] = section(b.uart_base, DEVICE);
    let mut i = 0;
    while i < b.device_sections.len() {
        let a = b.device_sections[i];
        t[(a >> 20) as usize] = section(a, DEVICE);
        i += 1;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{ARRIA10, ZYNQ7000};

    fn attr(t: &[u32; 4096], a: u32) -> u32 {
        t[(a >> 20) as usize] & 0xF_FFFF
    }

    #[test]
    fn identity_map_with_expected_attributes() {
        let t = table(&ZYNQ7000);
        assert_eq!(t[0x3E0], 0x3E00_0000 | PRIVATE);
        assert_eq!(attr(&t, board::FW_BASE + board::FW_SIZE - 1), PRIVATE);
        assert_eq!(attr(&t, board::RSC_BASE), SHARED_EXEC);
        assert_eq!(attr(&t, board::VRING0), SHARED);
        assert_eq!(attr(&t, board::BUF_BASE), SHARED);
        assert_eq!(attr(&t, 0xE000_0000), DEVICE);
        // Linux's memory and everything else faults.
        assert_eq!(t[0], 0);
        assert_eq!(t[0x100], 0);
        assert_eq!(t[0x400], 0);
        assert_eq!(t.iter().filter(|&&d| d != 0).count(), 15 + 1 + 16 + 1);
    }

    #[test]
    fn arria10_maps_its_uart() {
        let t = table(&ARRIA10);
        assert_eq!(attr(&t, 0xFFC0_2000), DEVICE);
        assert_eq!(attr(&t, 0xE000_0000), 0);
    }

    #[test]
    fn encodings() {
        // Section, AP=11, TEX=100, C=1, B=1 (inner WB no-WA, outer NC).
        assert_eq!(PRIVATE, 0x4C0E);
        // Section, AP=11, TEX=001, S, XN: normal non-cacheable.
        assert_eq!(SHARED, 0x1_1C12);
        // Section, AP=11, B, XN: shareable device.
        assert_eq!(DEVICE, 0x0C16);
    }
}
