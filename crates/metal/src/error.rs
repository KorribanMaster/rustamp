// SPDX-License-Identifier: BSD-3-Clause
use core::fmt;

/// Errors reported by the metal layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum Error {
    /// An offset or length falls outside the region.
    OutOfBounds,
    /// An access is not aligned to its natural size.
    Misaligned,
    /// A physical/device address does not map into the region.
    InvalidAddress,
    /// An allocator ran out of space.
    NoMemory,
    /// A parameter was invalid.
    InvalidParam,
    /// The operation is not supported by this backend.
    Unsupported,
    /// An operating-system call failed (Linux backend), with its `errno`.
    Os(i32),
}

/// Shorthand for `core::result::Result<T, Error>`.
pub type Result<T> = core::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::OutOfBounds => f.write_str("access outside the I/O region"),
            Error::Misaligned => f.write_str("misaligned access"),
            Error::InvalidAddress => f.write_str("address does not map into the region"),
            Error::NoMemory => f.write_str("out of memory"),
            Error::InvalidParam => f.write_str("invalid parameter"),
            Error::Unsupported => f.write_str("operation not supported"),
            Error::Os(e) => write!(f, "os error {e}"),
        }
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every message is non-empty and distinct, and a wrapped error's
    /// message includes the inner one.
    fn check_messages(all: &[Error]) {
        use std::string::ToString;
        let msgs: std::vec::Vec<_> = all.iter().map(|e| e.to_string()).collect();
        for (i, m) in msgs.iter().enumerate() {
            assert!(!m.is_empty(), "{:?}", all[i]);
            assert!(!msgs[..i].contains(m), "duplicate message {m:?}");
        }
    }

    #[test]
    fn messages() {
        use std::string::ToString;
        check_messages(&[
            Error::OutOfBounds,
            Error::Misaligned,
            Error::InvalidAddress,
            Error::NoMemory,
            Error::InvalidParam,
            Error::Unsupported,
            Error::Os(13),
        ]);
        assert!(Error::Os(13).to_string().contains("13"));
    }
}
