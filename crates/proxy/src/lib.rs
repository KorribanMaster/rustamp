// SPDX-License-Identifier: BSD-3-Clause
//! File and console I/O forwarding over rpmsg: a port of open-amp's
//! `lib/proxy` (`rpmsg_retarget`), wire-compatible with the C
//! implementation and `rpmsg-proxy-app`.
//!
//! Part of [openamp-rs](https://github.com/KorribanMaster/openamp-rs):
//!
//! ```text
//! openamp-proxy ──► openamp-rpmsg ──────┐
//!                                       ├──► openamp-virtio ──► openamp-metal
//! openamp-remoteproc ───────────────────┘
//!   (supplies the virtio Transport that rpmsg runs over)
//! ```
//!
//! In OpenAMP terms, the *host* here is the main controller (often Linux),
//! and the *remote* is the core it loads, starts and talks to.
//!
//! The remote has no filesystem. It sends `open`/`read`/`write`/`close`
//! requests to a proxy on the host, which executes them and replies.
//!
//! * [`ProxyClient`] (remote, `no_std`): the calls, and [`Stdout`] for
//!   `core::fmt::Write`. Runs over any [`Channel`], such as
//!   [`RpmsgChannel`] over an `openamp-rpmsg` endpoint.
//! * [`server::ProxyServer`] (host): executes requests inside a root
//!   directory it never lets the remote leave.
//!
//! ```no_run
//! # use openamp_proxy::{flags, Channel, ProxyClient};
//! # fn demo<C: Channel>(channel: C) -> Result<(), openamp_proxy::Error<C::Error>> {
//! use core::fmt::Write;
//!
//! let mut proxy = ProxyClient::new(channel);
//! let _ = writeln!(proxy.stdout(), "hello from the remote");
//! let fd = proxy.open("log.txt", flags::O_WRONLY | flags::O_CREAT, 0o644)?;
//! proxy.write_all(fd, b"written on the host\n")?;
//! proxy.close(fd)?;
//! # Ok(()) }
//! ```
//!
//! # Features
//!
//! | Feature | Adds |
//! |---|---|
//! | `rpmsg` (default) | [`RpmsgChannel`] |
//! | `std` | [`server::ProxyServer`] |
//! | `embedded-io` | [`ProxyClient::open_file`], a [`ProxyFile`] implementing `embedded_io::{Read, Write}` |
//! | `log`, `defmt` | Forwarded to the rpmsg stack; `defmt::Format` on [`Error`] |
#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(any(feature = "std", test))]
extern crate std;

mod client;
#[cfg(feature = "embedded-io")]
mod file;
#[cfg(feature = "rpmsg")]
mod rpmsg;
#[cfg(feature = "std")]
pub mod server;
pub mod wire;

pub use client::{Channel, Error, ProxyClient, Stdout};
#[cfg(feature = "embedded-io")]
pub use file::{FileError, ProxyFile};
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
