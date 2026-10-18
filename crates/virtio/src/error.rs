// SPDX-License-Identifier: BSD-3-Clause
use core::fmt;

/// Virtio errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum Error {
    /// A shared-memory access failed.
    Metal(openamp_metal::Error),
    /// The ring has no free descriptors.
    QueueFull,
    /// The ring size or alignment is invalid.
    BadLayout,
    /// The peer published an invalid descriptor index, chain or index jump.
    InvalidDescriptor,
    /// A queue index that the transport does not have.
    NoSuchQueue,
    /// The transport does not support the operation in this role.
    Unsupported,
    /// The device is not (yet) in the required state.
    NotReady,
    /// The device rejected the configuration.
    Rejected,
}

/// Shorthand for `core::result::Result<T, Error>`.
pub type Result<T> = core::result::Result<T, Error>;

impl From<openamp_metal::Error> for Error {
    fn from(e: openamp_metal::Error) -> Self {
        Error::Metal(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Metal(e) => write!(f, "shared memory: {e}"),
            Error::QueueFull => f.write_str("virtqueue full"),
            Error::BadLayout => f.write_str("invalid vring size or alignment"),
            Error::InvalidDescriptor => f.write_str("peer published an invalid descriptor"),
            Error::NoSuchQueue => f.write_str("no such virtqueue"),
            Error::Unsupported => f.write_str("operation not supported"),
            Error::NotReady => f.write_str("device not ready"),
            Error::Rejected => f.write_str("device rejected the configuration"),
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
    fn messages_and_conversion() {
        use std::string::ToString;
        let inner = openamp_metal::Error::Misaligned;
        check_messages(&[
            Error::Metal(inner),
            Error::QueueFull,
            Error::BadLayout,
            Error::InvalidDescriptor,
            Error::NoSuchQueue,
            Error::Unsupported,
            Error::NotReady,
            Error::Rejected,
        ]);
        assert!(Error::Metal(inner).to_string().contains(&inner.to_string()));
        assert_eq!(Error::from(inner), Error::Metal(inner));
    }
}
