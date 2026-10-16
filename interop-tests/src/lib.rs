// SPDX-License-Identifier: BSD-3-Clause
//! FFI to the C peers in `c/glue.c` and the shared-memory layout they use.

use core::ffi::c_void;

/// Offset of vring 0 in the shared buffer.
pub const VRING0_OFF: usize = 0x1000;
/// Offset of vring 1.
pub const VRING1_OFF: usize = 0x5000;
/// Start of the host buffer pool.
pub const POOL_OFF: usize = 0x10000;
/// Size of the shared buffer.
pub const SHM_SIZE: usize = 0x10_0000;
/// Physical address the buffer pretends to have.
pub const SHM_PHYS: u64 = 0x8000_0000;
/// Service both sides use.
pub const SERVICE: &str = "rpmsg-interop";

unsafe extern "C" {
    /// Writes the C remote's resource table at the start of `va`.
    pub fn c_remote_write_table(va: *mut c_void, pa: u64, vring_num: u32);
    /// Size of that table.
    pub fn c_rsc_table_size() -> usize;
    /// Runs the C remote (echo server) until "quit"; returns messages echoed.
    pub fn c_remote_run(va: *mut c_void, pa: u64, size: usize) -> i32;
    /// Runs the C host for `n` checked round trips; returns echoes verified.
    pub fn c_host_run(va: *mut c_void, pa: u64, size: usize, n: u32) -> i32;
    /// Notifications raised by the C remote (0) or host (1).
    pub fn c_peer_kicks(which: i32) -> i32;
}

/// The message the C host sends as number `seq` (mirrors `make_msg`).
pub fn make_msg(seq: u32, max: usize) -> Vec<u8> {
    let len = 4 + (seq as usize * 13) % (max - 4 + 1);
    let mut v = vec![0u8; len];
    v[..4].copy_from_slice(&seq.to_le_bytes());
    for (i, b) in v.iter_mut().enumerate().skip(4) {
        *b = (seq as usize + i) as u8;
    }
    v
}
