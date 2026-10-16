// SPDX-License-Identifier: BSD-3-Clause
//! File and console I/O forwarding over rpmsg, a Rust port of
//! `open-amp/lib/proxy` (`rpmsg_retarget`).
//!
//! The remote has no filesystem; it sends `open`/`read`/`write`/`close`
//! requests to a proxy on the host, which executes them and replies.
//!
//! * [`ProxyClient`] (remote, `no_std`): the calls, plus [`Stdout`] for
//!   `core::fmt::Write`. Runs over any [`Channel`]; with the `rpmsg`
//!   feature, [`RpmsgChannel`] uses an `openamp-rpmsg` endpoint.
//! * [`server::ProxyServer`] (host, `std` feature): executes requests inside
//!   a sandbox directory.
//!
//! The wire format is `struct rpmsg_rpc_syscall` from
//! `openamp/rpmsg_retarget.h`, so either side interoperates with the C
//! implementation (`rpmsg-proxy-app` from openamp-system-reference).
#![no_std]

#[cfg(any(feature = "std", test))]
extern crate std;

mod client;
#[cfg(feature = "rpmsg")]
mod rpmsg;
#[cfg(feature = "std")]
pub mod server;
pub mod wire;

pub use client::{Channel, Error, ProxyClient, Stdout};
#[cfg(feature = "rpmsg")]
pub use rpmsg::RpmsgChannel;

/// Endpoint name used by the C proxy demo.
pub const PROXY_ENDPOINT_NAME: &str = "rpmsg-openamp-demo-channel";
/// Default proxy endpoint address (`DEFAULT_PROXY_ENDPOINT`).
pub const DEFAULT_PROXY_ENDPOINT: u32 = 0xFF;
/// Largest request or response (`MAX_BUF_LEN`): a 512-byte rpmsg buffer
/// minus the rpmsg header.
pub const MAX_BUF_LEN: usize = 496;

/// `open(2)` flags in the encoding newlib uses on bare-metal remotes.
pub mod flags {
    /// Read only.
    pub const O_RDONLY: i32 = 0;
    /// Write only.
    pub const O_WRONLY: i32 = 1;
    /// Read and write.
    pub const O_RDWR: i32 = 2;
    /// Mask for the access mode.
    pub const O_ACCMODE: i32 = 3;
    /// Append.
    pub const O_APPEND: i32 = 0x0008;
    /// Create if missing.
    pub const O_CREAT: i32 = 0x0200;
    /// Truncate.
    pub const O_TRUNC: i32 = 0x0400;
    /// Fail if it exists (with `O_CREAT`).
    pub const O_EXCL: i32 = 0x0800;
}
