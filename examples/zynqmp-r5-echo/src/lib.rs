// SPDX-License-Identifier: BSD-3-Clause
//! Hardware-independent parts of the ZynqMP R5 firmware: the memory map and
//! the IPI and GIC register encodings. Kept in a library so they can be
//! unit-tested on the host; the firmware binary (`main.rs`) adds the parts
//! that touch hardware.
#![no_std]

pub mod board;
pub mod gic;
pub mod ipi;
