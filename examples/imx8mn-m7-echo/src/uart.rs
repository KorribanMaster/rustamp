// SPDX-License-Identifier: BSD-3-Clause
//! Polled transmit on i.MX UART4 (0x30A60000), for debug output under QEMU.
//!
//! Real silicon needs clocks and pin muxing first; QEMU's model only needs
//! the enable bits.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

const BASE: usize = 0x30A6_0000;
const UTXD: usize = 0x40;
const UCR1: usize = 0x80;
const UCR2: usize = 0x84;
const UTS: usize = 0xB4;
const UCR1_UARTEN: u32 = 1 << 0;
/// SRST (active low, so 1 = not in reset), RXEN, TXEN, WS (8 bits).
const UCR2_RUN: u32 = 1 << 0 | 1 << 1 | 1 << 2 | 1 << 5;
const UTS_TXFULL: u32 = 1 << 4;

fn reg(off: usize) -> *mut u32 {
    (BASE + off) as *mut u32
}

/// Enables the transmitter.
pub fn init() {
    // SAFETY: UART4 is reserved for the M7 in the device tree.
    unsafe {
        write_volatile(reg(UCR2), UCR2_RUN);
        write_volatile(reg(UCR1), UCR1_UARTEN);
    }
}

/// The UART as a `fmt::Write` sink.
pub struct Uart;

impl fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            // SAFETY: as in `init`.
            unsafe {
                while read_volatile(reg(UTS)) & UTS_TXFULL != 0 {}
                write_volatile(reg(UTXD), b as u32);
            }
        }
        Ok(())
    }
}

/// `println!`-style logging to UART4.
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {{
        let _ = core::fmt::Write::write_fmt(&mut $crate::uart::Uart, format_args!($($t)*));
        let _ = core::fmt::Write::write_str(&mut $crate::uart::Uart, "\r\n");
    }};
}
