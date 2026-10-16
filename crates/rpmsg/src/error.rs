// SPDX-License-Identifier: BSD-3-Clause
use core::fmt;

/// RPMsg errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Shared-memory access failed.
    Metal(openamp_metal::Error),
    /// The virtqueue layer failed (often: the peer published bad indices).
    Virtio(openamp_virtio::Error),
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
