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
