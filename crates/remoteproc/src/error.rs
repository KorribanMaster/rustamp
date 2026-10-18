// SPDX-License-Identifier: BSD-3-Clause
use core::fmt;

/// Remoteproc errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum Error {
    /// Shared-memory access failed.
    Metal(openamp_metal::Error),
    /// Virtio-level failure.
    Virtio(openamp_virtio::Error),
    /// The firmware image is malformed or truncated.
    InvalidImage,
    /// The image is valid but uses something unsupported (big-endian, ...).
    UnsupportedImage,
    /// The resource table is malformed.
    InvalidTable,
    /// The firmware has no resource table.
    NoResourceTable,
    /// A resource the host cannot satisfy (e.g. an unhandled vendor type).
    UnsupportedResource(u32),
    /// The operation is not allowed in the current lifecycle state.
    InvalidState,
    /// Out of memory for vrings or carve-outs.
    NoMemory,
    /// A platform hook failed with this code.
    Platform(i32),
}

/// Shorthand for `core::result::Result<T, Error>`.
pub type Result<T> = core::result::Result<T, Error>;

impl From<openamp_metal::Error> for Error {
    fn from(e: openamp_metal::Error) -> Self {
        Error::Metal(e)
    }
}

impl From<openamp_virtio::Error> for Error {
    fn from(e: openamp_virtio::Error) -> Self {
        Error::Virtio(e)
    }
}

impl From<Error> for openamp_virtio::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Metal(m) => openamp_virtio::Error::Metal(m),
            Error::Virtio(v) => v,
            _ => openamp_virtio::Error::NotReady,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Metal(e) => write!(f, "shared memory: {e}"),
            Error::Virtio(e) => write!(f, "virtio: {e}"),
            Error::InvalidImage => f.write_str("invalid firmware image"),
            Error::UnsupportedImage => f.write_str("unsupported firmware image"),
            Error::InvalidTable => f.write_str("invalid resource table"),
            Error::NoResourceTable => f.write_str("firmware has no resource table"),
            Error::UnsupportedResource(t) => write!(f, "unsupported resource type {t}"),
            Error::InvalidState => f.write_str("invalid remoteproc state"),
            Error::NoMemory => f.write_str("out of memory"),
            Error::Platform(c) => write!(f, "platform error {c}"),
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
    fn messages_and_conversions() {
        use std::string::ToString;
        let m = openamp_metal::Error::NoMemory;
        let v = openamp_virtio::Error::BadLayout;
        check_messages(&[
            Error::Metal(m),
            Error::Virtio(v),
            Error::InvalidImage,
            Error::UnsupportedImage,
            Error::InvalidTable,
            Error::NoResourceTable,
            Error::UnsupportedResource(129),
            Error::InvalidState,
            Error::NoMemory,
            Error::Platform(-5),
        ]);
        assert!(Error::UnsupportedResource(129).to_string().contains("129"));
        assert!(Error::Platform(-5).to_string().contains("-5"));
        assert_eq!(Error::from(m), Error::Metal(m));
        assert_eq!(Error::from(v), Error::Virtio(v));
        // Back into virtio errors, for the Transport implementation.
        assert_eq!(
            openamp_virtio::Error::from(Error::Metal(m)),
            openamp_virtio::Error::Metal(m)
        );
        assert_eq!(openamp_virtio::Error::from(Error::Virtio(v)), v);
        assert_eq!(
            openamp_virtio::Error::from(Error::InvalidTable),
            openamp_virtio::Error::NotReady
        );
    }
}
