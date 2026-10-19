// SPDX-License-Identifier: BSD-3-Clause
//! Polled debug output on a Cadence UART (`xuartps`), also copied into the
//! remoteproc trace buffer.
//!
//! The UART must already be clocked and its baud rate set (by the boot
//! firmware; with Linux running, `clk_ignore_unused` keeps an unused UART's
//! clock on). This code only enables the transmitter and writes.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

use zynqmp_r5_echo::board::UART_BASE;

const CR: u32 = 0x00;
const SR: u32 = 0x2C;
const FIFO: u32 = 0x30;
const CR_TXEN: u32 = 1 << 4;
const CR_RXEN: u32 = 1 << 2;
const SR_TXFULL: u32 = 1 << 4;

fn reg(off: u32) -> *mut u32 {
    (UART_BASE + off) as usize as *mut u32
}

/// Enables the transmitter.
pub fn init() {
    // SAFETY: the UART is left to the R5 (Linux does not use it).
    unsafe { write_volatile(reg(CR), CR_TXEN | CR_RXEN) };
}

fn put(b: u8) {
    // SAFETY: as in `init`.
    unsafe {
        while read_volatile(reg(SR)) & SR_TXFULL != 0 {}
        write_volatile(reg(FIFO), b as u32);
    }
    crate::trace::put(b);
}

/// The UART (and trace buffer) as a `fmt::Write` sink.
pub struct Uart;

impl fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        s.bytes().for_each(put);
        Ok(())
    }
}

/// `println!`-style logging to the debug UART and the trace buffer.
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {{
        let _ = core::fmt::Write::write_fmt(&mut $crate::uart::Uart, format_args!($($t)*));
        let _ = core::fmt::Write::write_str(&mut $crate::uart::Uart, "\r\n");
    }};
}
