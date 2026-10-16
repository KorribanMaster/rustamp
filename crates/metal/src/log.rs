// SPDX-License-Identifier: BSD-3-Clause
//! Diagnostics (`metal_log`).
//!
//! Like libmetal, logging goes to a handler the application installs with
//! [`set_handler`]; with no handler installed, the macros cost one relaxed
//! atomic load. On a host, [`stderr_handler`] prints to standard error; on
//! firmware, forward to RTT, a UART or `defmt`.

use core::fmt::Arguments;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

/// Severity of a log message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// Something failed.
    Error = 1,
    /// Something unexpected that was handled.
    Warn = 2,
    /// Informational.
    Info = 3,
    /// Debugging detail.
    Debug = 4,
}

/// A log handler.
pub type Handler = fn(Level, Arguments<'_>);

static HANDLER: AtomicUsize = AtomicUsize::new(0);
static MAX_LEVEL: AtomicU8 = AtomicU8::new(Level::Warn as u8);

/// Installs the log handler.
pub fn set_handler(handler: Handler) {
    HANDLER.store(handler as usize, Ordering::Release);
}

/// Sets the most verbose level that reaches the handler (default `Warn`).
pub fn set_level(level: Level) {
    MAX_LEVEL.store(level as u8, Ordering::Relaxed);
}

#[doc(hidden)]
#[inline]
pub fn __log(level: Level, args: Arguments<'_>) {
    if level as u8 > MAX_LEVEL.load(Ordering::Relaxed) {
        return;
    }
    let h = HANDLER.load(Ordering::Acquire);
    if h != 0 {
        // SAFETY: non-zero values are only stored from a `Handler`.
        let h: Handler = unsafe { core::mem::transmute::<usize, Handler>(h) };
        h(level, args);
    }
}

/// A handler printing to standard error.
#[cfg(feature = "std")]
pub fn stderr_handler(level: Level, args: Arguments<'_>) {
    std::eprintln!("[openamp {level:?}] {args}");
}

/// Logs at error level.
#[macro_export]
macro_rules! metal_error { ($($t:tt)*) => { $crate::log::__log($crate::log::Level::Error, ::core::format_args!($($t)*)) } }
/// Logs at warn level.
#[macro_export]
macro_rules! metal_warn { ($($t:tt)*) => { $crate::log::__log($crate::log::Level::Warn, ::core::format_args!($($t)*)) } }
/// Logs at info level.
#[macro_export]
macro_rules! metal_info { ($($t:tt)*) => { $crate::log::__log($crate::log::Level::Info, ::core::format_args!($($t)*)) } }
/// Logs at debug level.
#[macro_export]
macro_rules! metal_debug { ($($t:tt)*) => { $crate::log::__log($crate::log::Level::Debug, ::core::format_args!($($t)*)) } }
