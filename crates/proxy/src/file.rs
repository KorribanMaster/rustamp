// SPDX-License-Identifier: BSD-3-Clause
//! Host files as [`embedded_io`] streams.

use core::fmt;

use crate::client::{Channel, Error, ProxyClient};

/// An open host file, closed when dropped.
///
/// Returned by [`ProxyClient::open_file`]. It implements
/// [`embedded_io::Read`] and [`embedded_io::Write`], so any `embedded-io`
/// consumer can use host files.
pub struct ProxyFile<'a, C: Channel> {
    client: &'a mut ProxyClient<C>,
    fd: i32,
}

impl<C: Channel> fmt::Debug for ProxyFile<'_, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyFile").field("fd", &self.fd).finish()
    }
}

impl<C: Channel> ProxyClient<C> {
    /// Opens `path` on the host as a [`ProxyFile`].
    pub fn open_file(
        &mut self,
        path: &str,
        flags: i32,
        mode: i32,
    ) -> Result<ProxyFile<'_, C>, Error<C::Error>> {
        let fd = self.open(path, flags, mode)?;
        Ok(ProxyFile { client: self, fd })
    }
}

impl<C: Channel> ProxyFile<'_, C> {
    /// The host file descriptor.
    pub fn fd(&self) -> i32 {
        self.fd
    }

    /// Closes the file, reporting errors that dropping it would ignore.
    pub fn close(mut self) -> Result<(), Error<C::Error>> {
        let fd = core::mem::replace(&mut self.fd, CLOSED);
        self.client.close(fd)
    }
}

/// `fd` after an explicit [`ProxyFile::close`]; host descriptors are never
/// negative.
const CLOSED: i32 = -1;

impl<C: Channel> Drop for ProxyFile<'_, C> {
    fn drop(&mut self) {
        if self.fd != CLOSED {
            let _ = self.client.close(self.fd);
        }
    }
}

/// Error of [`ProxyFile`] I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FileError<E> {
    /// The proxy call failed.
    Proxy(Error<E>),
    /// The host accepted no bytes of a non-empty write.
    WriteZero,
}

impl<E> From<Error<E>> for FileError<E> {
    fn from(e: Error<E>) -> Self {
        FileError::Proxy(e)
    }
}

impl<E: fmt::Debug> fmt::Display for FileError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileError::Proxy(e) => e.fmt(f),
            FileError::WriteZero => f.write_str("host wrote no bytes"),
        }
    }
}

impl<E: fmt::Debug> core::error::Error for FileError<E> {}

impl<E: fmt::Debug> embedded_io::Error for FileError<E> {
    fn kind(&self) -> embedded_io::ErrorKind {
        match self {
            // ENOENT
            FileError::Proxy(Error::Remote(-2)) => embedded_io::ErrorKind::NotFound,
            FileError::WriteZero => embedded_io::ErrorKind::WriteZero,
            FileError::Proxy(_) => embedded_io::ErrorKind::Other,
        }
    }
}

impl<C: Channel<Error: fmt::Debug>> embedded_io::ErrorType for ProxyFile<'_, C> {
    type Error = FileError<C::Error>;
}

impl<C: Channel<Error: fmt::Debug>> embedded_io::Read for ProxyFile<'_, C> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        Ok(self.client.read(self.fd, buf)?)
    }
}

impl<C: Channel<Error: fmt::Debug>> embedded_io::Write for ProxyFile<'_, C> {
    /// Writes one proxy message's worth of `buf`.
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.client.write(self.fd, buf)? {
            0 => Err(FileError::WriteZero),
            n => Ok(n),
        }
    }

    /// A no-op: the host writes synchronously.
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
