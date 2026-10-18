// SPDX-License-Identifier: BSD-3-Clause
//! Logging macros forwarding to `defmt` or `log`, by feature.
//!
//! `defmt` macros resolve paths in the calling crate, so each crate that logs
//! carries its own copy of this file. With neither feature the macros compile
//! to nothing (the arguments are still type-checked). If both are enabled,
//! which feature unification can do, `log` is used.
#![allow(unused_macros)]

macro_rules! __fmt_forward {
    ($level:ident, $s:literal $(, $x:expr)* $(,)?) => {{
        #[cfg(feature = "log")]
        ::log::$level!($s $(, $x)*);
        #[cfg(all(feature = "defmt", not(feature = "log")))]
        ::defmt::$level!($s $(, $x)*);
        #[cfg(not(any(feature = "log", feature = "defmt")))]
        let _ = ($(&$x,)*);
    }};
}

macro_rules! error { ($($t:tt)*) => { __fmt_forward!(error, $($t)*) }; }
macro_rules! warn { ($($t:tt)*) => { __fmt_forward!(warn, $($t)*) }; }
macro_rules! debug { ($($t:tt)*) => { __fmt_forward!(debug, $($t)*) }; }
macro_rules! trace { ($($t:tt)*) => { __fmt_forward!(trace, $($t)*) }; }
