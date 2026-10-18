// SPDX-License-Identifier: BSD-3-Clause
//! Hardware-independent parts of the Cortex-A9 RTU firmware: the memory map,
//! the MMU table, GIC register encodings and the park-page protocol shared
//! with the Linux driver (`linux/a9-rproc`). Kept in a library so they can be
//! unit-tested on the host; the firmware binary (`main.rs`) adds the parts
//! that touch hardware.
#![no_std]

pub mod board;
pub mod gic;
pub mod mmu;
pub mod park;
