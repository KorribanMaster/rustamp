// SPDX-License-Identifier: BSD-3-Clause
use core::fmt;

/// RPMsg errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub enum Error {
    /// Shared-memory access failed.
    Metal(rustamp_metal::Error),
    /// The virtqueue layer failed (often: the peer published bad indices).
    Virtio(rustamp_virtio::Error),
    /// No transmit buffer is free (`RPMSG_ERR_NO_BUFF`).
    NoBuffer,
    /// The payload does not fit in a buffer (`RPMSG_ERR_BUFF_SIZE`).
    MessageTooLarge,
    /// The endpoint table is full (`RPMSG_ERR_NO_MEM`).
    NoEndpointSlot,
    /// The address is taken or outside the allocatable range
    /// (`RPMSG_ERR_ADDR`).
    InvalidAddress,
    /// The endpoint handle does not refer to a live endpoint.
    InvalidEndpoint,
    /// The device is not ready, or the endpoint has no destination yet
    /// (`RPMSG_ERR_DEV_STATE`).
    NotReady,
    /// A parameter is invalid (`RPMSG_ERR_PARAM`).
    InvalidParam,
    /// A buffer handed over by the peer does not lie in shared memory.
    InvalidBuffer,
    /// Waiting for a buffer timed out.
    Timeout,
}

/// Shorthand for `core::result::Result<T, Error>`.
pub type Result<T> = core::result::Result<T, Error>;

impl From<rustamp_metal::Error> for Error {
    fn from(e: rustamp_metal::Error) -> Self {
        Error::Metal(e)
    }
}

impl From<rustamp_virtio::Error> for Error {
    fn from(e: rustamp_virtio::Error) -> Self {
        Error::Virtio(e)
    }
}

impl Error {
    /// The equivalent `RPMSG_ERR_*` code of the C library.
    pub fn to_c(self) -> i32 {
        const BASE: i32 = -2000;
        match self {
            Error::NoEndpointSlot => BASE - 1,
            Error::NoBuffer | Error::Timeout => BASE - 2,
            Error::InvalidParam | Error::InvalidEndpoint => BASE - 3,
            Error::NotReady => BASE - 4,
            Error::MessageTooLarge => BASE - 5,
            Error::Metal(_) | Error::Virtio(_) | Error::InvalidBuffer => BASE - 6,
            Error::InvalidAddress => BASE - 7,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Metal(e) => write!(f, "shared memory: {e}"),
            Error::Virtio(e) => write!(f, "virtio: {e}"),
            Error::NoBuffer => f.write_str("no transmit buffer available"),
            Error::MessageTooLarge => f.write_str("message larger than buffer"),
            Error::NoEndpointSlot => f.write_str("endpoint table full"),
            Error::InvalidAddress => f.write_str("endpoint address unavailable"),
            Error::InvalidEndpoint => f.write_str("stale or invalid endpoint"),
            Error::NotReady => f.write_str("device or endpoint not ready"),
            Error::InvalidParam => f.write_str("invalid parameter"),
            Error::InvalidBuffer => f.write_str("peer buffer outside shared memory"),
            Error::Timeout => f.write_str("timed out waiting for a buffer"),
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

    const ALL: [Error; 11] = [
        Error::Metal(rustamp_metal::Error::OutOfBounds),
        Error::Virtio(rustamp_virtio::Error::QueueFull),
        Error::NoBuffer,
        Error::MessageTooLarge,
        Error::NoEndpointSlot,
        Error::InvalidAddress,
        Error::InvalidEndpoint,
        Error::NotReady,
        Error::InvalidParam,
        Error::InvalidBuffer,
        Error::Timeout,
    ];

    #[test]
    fn messages_and_conversions() {
        use std::string::ToString;
        check_messages(&ALL);
        let m = rustamp_metal::Error::OutOfBounds;
        let v = rustamp_virtio::Error::QueueFull;
        assert_eq!(Error::from(m), Error::Metal(m));
        assert_eq!(Error::from(v), Error::Virtio(v));
        assert!(Error::Virtio(v).to_string().contains(&v.to_string()));
    }

    /// The codes of `RPMSG_ERR_*` in open-amp's `rpmsg.h`.
    #[test]
    fn c_error_codes() {
        let base = -2000;
        let code = |e: Error| e.to_c() - base;
        assert_eq!(code(Error::NoEndpointSlot), -1); // RPMSG_ERR_NO_MEM
        assert_eq!(code(Error::NoBuffer), -2); // RPMSG_ERR_NO_BUFF
        assert_eq!(code(Error::Timeout), -2);
        assert_eq!(code(Error::InvalidParam), -3); // RPMSG_ERR_PARAM
        assert_eq!(code(Error::InvalidEndpoint), -3);
        assert_eq!(code(Error::NotReady), -4); // RPMSG_ERR_DEV_STATE
        assert_eq!(code(Error::MessageTooLarge), -5); // RPMSG_ERR_BUFF_SIZE
        assert_eq!(code(Error::InvalidBuffer), -6); // RPMSG_ERR_INIT
        assert_eq!(code(ALL[0]), -6);
        assert_eq!(code(ALL[1]), -6);
        assert_eq!(code(Error::InvalidAddress), -7); // RPMSG_ERR_ADDR
    }
}
