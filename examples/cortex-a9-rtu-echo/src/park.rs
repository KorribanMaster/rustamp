// SPDX-License-Identifier: BSD-3-Clause
//! The park-page protocol shared with the Linux driver (`a9-rproc`).
//!
//! Not every A9 SoC lets Linux hold a single core in reset (and QEMU's Zynq
//! model ignores the SLCR reset bits), so stopping is cooperative:
//!
//! 1. Linux writes [`STOP_REQ`] = 1 and sends the RTU its SGI.
//! 2. The RTU copies a tiny stub into the park page, turns its MMU and
//!    caches off and jumps there. The stub writes [`MAGIC`] to
//!    [`MAGIC_OFF`] and waits (`wfi`) for a non-zero entry address in
//!    [`RELEASE_OFF`], acknowledging SGIs as they come.
//! 3. Linux sees the magic, may now overwrite the firmware image, and may
//!    also put the core into reset if the SoC supports it.
//! 4. To start again, Linux writes the new entry to [`RELEASE_OFF`] and
//!    sends the SGI; the stub clears the page's words and jumps.
//!
//! The park page is the last 4 KiB of the resource-table section, which no
//! firmware segment covers, so a reload never overwrites the running stub.

/// Offset of the stop-request word (Linux writes 1).
pub const STOP_REQ_OFF: u32 = 0xFF0;
/// Offset of the "parked" magic word (the stub writes [`MAGIC`]).
pub const MAGIC_OFF: u32 = 0xFF8;
/// Offset of the release word (Linux writes the entry address).
pub const RELEASE_OFF: u32 = 0xFFC;
/// "PARK", little-endian.
pub const MAGIC: u32 = u32::from_le_bytes(*b"PARK");
/// Value of the stop request.
pub const STOP_REQ: u32 = 1;
/// Room for the stub's code in front of the protocol words.
pub const STUB_MAX: u32 = STOP_REQ_OFF;

// The protocol words are in order and inside the page (checked at build time).
const _: () = assert!(STOP_REQ_OFF < MAGIC_OFF && MAGIC_OFF < RELEASE_OFF);
const _: () = assert!(RELEASE_OFF + 4 <= 0x1000);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_matches_the_driver() {
        // a9_rproc.c: PARK_MAGIC
        assert_eq!(MAGIC, 0x4B52_4150);
    }
}
