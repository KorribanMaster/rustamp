// SPDX-License-Identifier: BSD-3-Clause
//! Polled debug output on the board's spare UART.
//!
//! The UART must already be clocked (Linux: `clk_ignore_unused`, or a clock
//! consumer in the device tree) and, on real hardware, its baud rate set by
//! the boot loader. This code only enables the transmitter and writes.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};

use cortex_a9_rtu_echo::board::{BOARD, UartKind};

fn reg(off: u32) -> *mut u32 {
    (BOARD.uart_base + off) as usize as *mut u32
}

// Cadence (Zynq xuartps).
const CDNS_CR: u32 = 0x00;
const CDNS_SR: u32 = 0x2C;
const CDNS_FIFO: u32 = 0x30;
const CDNS_CR_TXEN: u32 = 1 << 4;
const CDNS_CR_RXEN: u32 = 1 << 2;
const CDNS_SR_TXFULL: u32 = 1 << 4;

// 16550 with 32-bit register stride (Arria 10).
const NS_THR: u32 = 0x00;
const NS_LSR: u32 = 0x14;
const NS_LSR_THRE: u32 = 1 << 5;

/// Enables the transmitter.
pub fn init() {
    if BOARD.uart == UartKind::Cadence {
        // SAFETY: the UART is reserved for the RTU (not used by Linux).
        unsafe { write_volatile(reg(CDNS_CR), CDNS_CR_TXEN | CDNS_CR_RXEN) };
    }
}

fn put(b: u8) {
    // SAFETY: as in `init`.
    unsafe {
        match BOARD.uart {
            UartKind::Cadence => {
                while read_volatile(reg(CDNS_SR)) & CDNS_SR_TXFULL != 0 {}
                write_volatile(reg(CDNS_FIFO), b as u32);
            }
            UartKind::Ns16550 => {
                while read_volatile(reg(NS_LSR)) & NS_LSR_THRE == 0 {}
                write_volatile(reg(NS_THR), b as u32);
            }
        }
    }
}

/// The UART as a `fmt::Write` sink.
pub struct Uart;

impl fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        s.bytes().for_each(put);
        Ok(())
    }
}

/// `println!`-style logging to the debug UART.
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {{
        let _ = core::fmt::Write::write_fmt(&mut $crate::uart::Uart, format_args!($($t)*));
        let _ = core::fmt::Write::write_str(&mut $crate::uart::Uart, "\r\n");
    }};
}
