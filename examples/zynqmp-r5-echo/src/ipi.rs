// SPDX-License-Identifier: BSD-3-Clause
//! Zynq UltraScale+ IPI (inter-processor interrupt) registers.
//!
//! Each agent has a channel. Writing a destination's mask to our `TRIG`
//! raises its interrupt; the destination sees our mask in its `ISR` and
//! acknowledges by writing it back, which clears our `OBS` bit. Linux's
//! mailbox driver waits for that acknowledgement before its next kick.

/// Trigger: write destination masks.
pub const TRIG: u32 = 0x00;
/// Observation: our triggers not yet acknowledged.
pub const OBS: u32 = 0x04;
/// Interrupt status: sources that triggered us (write 1 to clear).
pub const ISR: u32 = 0x10;
/// Interrupt mask (read-only).
pub const IMR: u32 = 0x14;
/// Interrupt enable (write 1 to enable a source).
pub const IER: u32 = 0x18;
/// Interrupt disable.
pub const IDR: u32 = 0x1C;

/// The `ISR` bits from `source`, to acknowledge.
pub const fn pending_from(isr: u32, source: u32) -> u32 {
    isr & source
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_offsets_match_ug1085() {
        assert_eq!(
            [TRIG, OBS, ISR, IMR, IER, IDR],
            [0x0, 0x4, 0x10, 0x14, 0x18, 0x1C]
        );
    }

    #[test]
    fn only_the_host_bits_are_acknowledged() {
        assert_eq!(pending_from(0x0100_0101, 0x1), 0x1);
        assert_eq!(pending_from(0x0100_0100, 0x1), 0);
    }
}
