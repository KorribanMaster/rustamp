// SPDX-License-Identifier: BSD-3-Clause
//! The remote side: `_open`, `_read`, `_write`, `_close` from
//! `rpmsg_retarget.c`.

use core::fmt;

use crate::MAX_BUF_LEN;
use crate::wire::{
    CLOSE_SYSCALL_ID, OPEN_SYSCALL_ID, READ_SYSCALL_ID, SYSCALL_SIZE, Syscall, TERM_SYSCALL_ID,
    WRITE_SYSCALL_ID,
};

/// A request/response transport to the proxy server
/// (`rpmsg_rpc_send`).
pub trait Channel {
    /// Transport error.
    type Error;

    /// Sends `req` and waits for the reply, copied into `resp`; returns the
    /// reply length (truncated to `resp.len()`).
    fn request(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize, Self::Error>;
}

/// Proxy errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// The channel failed.
    Channel(E),
    /// The arguments do not fit in one message.
    TooLong,
    /// The reply was malformed or for another call.
    Protocol,
    /// The host returned a negative value (an `errno`, negated).
    Remote(i32),
    /// The host asked the proxy to terminate.
    Terminated,
}

impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Channel(e) => write!(f, "channel: {e:?}"),
            Error::TooLong => f.write_str("arguments too long"),
            Error::Protocol => f.write_str("malformed proxy reply"),
            Error::Remote(c) => write!(f, "host returned {c}"),
            Error::Terminated => f.write_str("proxy terminated by host"),
        }
    }
}

/// The remote end of the proxy.
#[derive(Debug)]
pub struct ProxyClient<C: Channel> {
    ch: C,
}

impl<C: Channel> ProxyClient<C> {
    /// A client over `ch`.
    pub fn new(ch: C) -> Self {
        ProxyClient { ch }
    }

    /// The channel.
    pub fn into_inner(self) -> C {
        self.ch
    }

    fn call(
        &mut self,
        id: u32,
        f1: i32,
        f2: i32,
        data: &[u8],
        nul: bool,
        resp: &mut [u8],
    ) -> Result<(Syscall, usize), Error<C::Error>> {
        let data_len = data.len() + nul as usize;
        if SYSCALL_SIZE + data_len > MAX_BUF_LEN {
            return Err(Error::TooLong);
        }
        let mut req = [0u8; MAX_BUF_LEN];
        let hdr = Syscall {
            id,
            int_field1: f1,
            int_field2: f2,
            data_len: data_len as u32,
        };
        req[..SYSCALL_SIZE].copy_from_slice(&hdr.to_bytes());
        req[SYSCALL_SIZE..SYSCALL_SIZE + data.len()].copy_from_slice(data);
        let n = self
            .ch
            .request(&req[..SYSCALL_SIZE + data_len], resp)
            .map_err(Error::Channel)?;
        let r = Syscall::from_bytes(&resp[..n]).ok_or(Error::Protocol)?;
        if r.id == TERM_SYSCALL_ID {
            return Err(Error::Terminated);
        }
        if r.id != id {
            return Err(Error::Protocol);
        }
        if r.int_field1 < 0 {
            return Err(Error::Remote(r.int_field1));
        }
        Ok((r, n))
    }

    /// Opens `path` on the host; returns the file descriptor.
    pub fn open(&mut self, path: &str, flags: i32, mode: i32) -> Result<i32, Error<C::Error>> {
        if path.as_bytes().contains(&0) {
            return Err(Error::Protocol);
        }
        let mut resp = [0u8; SYSCALL_SIZE];
        let (r, _) = self.call(
            OPEN_SYSCALL_ID,
            flags,
            mode,
            path.as_bytes(),
            true,
            &mut resp,
        )?;
        Ok(r.int_field1)
    }

    /// Closes `fd`.
    pub fn close(&mut self, fd: i32) -> Result<(), Error<C::Error>> {
        let mut resp = [0u8; SYSCALL_SIZE];
        self.call(CLOSE_SYSCALL_ID, fd, 0, &[], false, &mut resp)?;
        Ok(())
    }

    /// Writes `data` (at most one message's worth) to `fd`; returns the
    /// number of bytes written. Use [`write_all`](Self::write_all) for
    /// longer data.
    pub fn write(&mut self, fd: i32, data: &[u8]) -> Result<usize, Error<C::Error>> {
        // stdout writes carry a trailing NUL, as in C.
        let nul = fd == 1;
        let max = MAX_BUF_LEN - SYSCALL_SIZE - nul as usize;
        let data = &data[..data.len().min(max)];
        let mut resp = [0u8; SYSCALL_SIZE];
        let (r, _) = self.call(
            WRITE_SYSCALL_ID,
            fd,
            data.len() as i32,
            data,
            nul,
            &mut resp,
        )?;
        Ok(r.int_field1 as usize)
    }

    /// Writes all of `data` to `fd`.
    pub fn write_all(&mut self, fd: i32, mut data: &[u8]) -> Result<(), Error<C::Error>> {
        while !data.is_empty() {
            let n = self.write(fd, data)?;
            if n == 0 {
                return Err(Error::Protocol);
            }
            data = &data[n..];
        }
        Ok(())
    }

    /// Reads up to `buf.len()` bytes from `fd`.
    pub fn read(&mut self, fd: i32, buf: &mut [u8]) -> Result<usize, Error<C::Error>> {
        let want = buf.len().min(MAX_BUF_LEN - SYSCALL_SIZE) as i32;
        let mut resp = [0u8; MAX_BUF_LEN];
        let (r, n) = self.call(READ_SYSCALL_ID, fd, want, &[], false, &mut resp)?;
        let got = (r.int_field1 as usize)
            .min(r.data_len as usize)
            .min(n - SYSCALL_SIZE)
            .min(buf.len());
        buf[..got].copy_from_slice(&resp[SYSCALL_SIZE..SYSCALL_SIZE + got]);
        Ok(got)
    }

    /// A `core::fmt::Write` adapter for the host's standard output.
    pub fn stdout(&mut self) -> Stdout<'_, C> {
        Stdout { client: self }
    }
}

/// Formats onto the host's standard output (`printf` via the proxy).
pub struct Stdout<'a, C: Channel> {
    client: &'a mut ProxyClient<C>,
}

impl<C: Channel> fmt::Write for Stdout<'_, C> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.client
            .write_all(1, s.as_bytes())
            .map_err(|_| fmt::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Records requests and replies with a canned response.
    struct Canned {
        reqs: Vec<Vec<u8>>,
        reply: Vec<u8>,
    }

    impl Channel for Canned {
        type Error = ();
        fn request(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize, ()> {
            self.reqs.push(req.to_vec());
            let n = self.reply.len().min(resp.len());
            resp[..n].copy_from_slice(&self.reply[..n]);
            Ok(n)
        }
    }

    fn reply(id: u32, ret: i32, data: &[u8]) -> Vec<u8> {
        let mut v = Syscall {
            id,
            int_field1: ret,
            int_field2: 0,
            data_len: data.len() as u32,
        }
        .to_bytes()
        .to_vec();
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn open_encodes_path_with_nul() {
        let mut c = ProxyClient::new(Canned {
            reqs: Vec::new(),
            reply: reply(OPEN_SYSCALL_ID, 5, &[]),
        });
        assert_eq!(c.open("log.txt", 0x201, 0o644), Ok(5));
        let req = &c.ch.reqs[0];
        let h = Syscall::from_bytes(req).unwrap();
        assert_eq!(
            (h.id, h.int_field1, h.int_field2, h.data_len),
            (1, 0x201, 0o644, 8)
        );
        assert_eq!(&req[16..], b"log.txt\0");
    }

    #[test]
    fn stdout_write_appends_nul_and_chunks() {
        let mut c = ProxyClient::new(Canned {
            reqs: Vec::new(),
            reply: reply(WRITE_SYSCALL_ID, 479, &[]),
        });
        let long = [b'x'; 600];
        c.write_all(1, &long[..479]).unwrap();
        let req = &c.ch.reqs[0];
        assert_eq!(req.len(), MAX_BUF_LEN);
        assert_eq!(*req.last().unwrap(), 0);
        let h = Syscall::from_bytes(req).unwrap();
        assert_eq!((h.int_field2, h.data_len), (479, 480));
    }

    #[test]
    fn errors_and_termination() {
        let mut c = ProxyClient::new(Canned {
            reqs: Vec::new(),
            reply: reply(OPEN_SYSCALL_ID, -2, &[]),
        });
        assert_eq!(c.open("missing", 0, 0), Err(Error::Remote(-2)));
        c.ch.reply = reply(TERM_SYSCALL_ID, 0, &[]);
        assert_eq!(c.close(3), Err(Error::Terminated));
        c.ch.reply = reply(READ_SYSCALL_ID, 0, &[]);
        assert_eq!(c.close(3), Err(Error::Protocol));
        assert_eq!(c.open(&"p".repeat(480), 0, 0), Err(Error::TooLong));
    }

    #[test]
    fn read_copies_reply_data() {
        let mut c = ProxyClient::new(Canned {
            reqs: Vec::new(),
            reply: reply(READ_SYSCALL_ID, 5, b"hello"),
        });
        let mut buf = [0u8; 3];
        assert_eq!(c.read(4, &mut buf), Ok(3));
        assert_eq!(&buf, b"hel");
    }
}
