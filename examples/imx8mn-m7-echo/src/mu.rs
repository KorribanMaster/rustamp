// SPDX-License-Identifier: BSD-3-Clause
//! The i.MX 8M Messaging Unit, B side (the M7's end).
//!
//! Linux `imx_rproc` uses the "tx" mailbox channel (A-side TR1 → our RR1)
//! to kick us with `vqid << 16`, and listens on "rx" (our TR1 → A-side RR1)
//! for our kicks. It services all vrings on any message, so the value we send
//! is informational. This matches NXP's rpmsg-lite port (`RPMSG_MU_CHANNEL`
//! = 1) and the `mboxes = <&mu 0 1>, <&mu 1 1>` binding.

// The QEMU build polls the MU and never sleeps on an interrupt.
#![cfg_attr(feature = "qemu-imx8mp", allow(dead_code))]

use core::ptr::{read_volatile, write_volatile};

/// MU B-side base address on i.MX 8M Nano (MU1_B, M7 view).
pub const MU_B_BASE: usize = 0x30AB_0000;
/// NVIC interrupt number of the MU on the M7 (`MU_M7_IRQn` in NXP's
/// `MIMX8MN6_cm7.h`; check it against your SDK headers).
pub const MU_IRQ: Irq = Irq(97);

/// A device interrupt number, for `cortex_m::peripheral::NVIC`.
#[derive(Clone, Copy)]
pub struct Irq(pub u16);

// SAFETY: `Irq` values are device interrupt numbers below the M7's 240.
unsafe impl cortex_m::interrupt::InterruptNumber for Irq {
    fn number(self) -> u16 {
        self.0
    }
}

const TR1: usize = 0x04;
const RR1: usize = 0x14;
const SR: usize = 0x20;
const CR: usize = 0x24;
/// Receive register 1 full (RF1).
const SR_RF1: u32 = 1 << 26;
/// Transmit register 1 empty (TE1).
const SR_TE1: u32 = 1 << 22;
/// Receive interrupt enable for RR1 (RIE1).
const CR_RIE1: u32 = 1 << 26;

/// The Messaging Unit registers.
#[derive(Clone, Copy)]
pub struct Mu {
    base: usize,
}

impl Mu {
    /// The MU at `base`.
    pub const fn new(base: usize) -> Self {
        Mu { base }
    }

    fn reg(&self, off: usize) -> *mut u32 {
        (self.base + off) as *mut u32
    }

    /// Enables the "message received on channel 1" interrupt.
    pub fn enable_rx_irq(&self) {
        // SAFETY: MU registers are always mapped on this SoC; CR bits other
        // than RIE1 are preserved.
        unsafe { write_volatile(self.reg(CR), read_volatile(self.reg(CR)) | CR_RIE1) }
    }

    /// Sends `msg` on channel 1, waiting for the previous one to be read.
    pub fn send(&self, msg: u32) {
        // SAFETY: as above.
        unsafe {
            while read_volatile(self.reg(SR)) & SR_TE1 == 0 {
                core::hint::spin_loop();
            }
            write_volatile(self.reg(TR1), msg);
        }
    }

    /// Takes a pending channel-1 message, if any (reading RR1 clears RF1).
    pub fn try_recv(&self) -> Option<u32> {
        // SAFETY: as above.
        unsafe {
            if read_volatile(self.reg(SR)) & SR_RF1 != 0 {
                Some(read_volatile(self.reg(RR1)))
            } else {
                None
            }
        }
    }
}
