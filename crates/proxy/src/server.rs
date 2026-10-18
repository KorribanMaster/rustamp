// SPDX-License-Identifier: BSD-3-Clause
//! The host side: executes proxy requests (`rpmsg-proxy-app`).
//!
//! Unlike the C demo, which passes paths and flags straight to `open(2)`,
//! the server confines the remote to a root directory and translates the
//! remote's `open` flags from the newlib encoding (selectable with
//! [`FlagEncoding`]).

use std::boxed::Box;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::vec::Vec;

use crate::MAX_BUF_LEN;
use crate::flags::{O_ACCMODE, O_APPEND, O_CREAT, O_EXCL, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY};
use crate::wire::{
    CLOSE_SYSCALL_ID, OPEN_SYSCALL_ID, READ_SYSCALL_ID, SYSCALL_SIZE, Syscall, TERM_SYSCALL_ID,
    WRITE_SYSCALL_ID,
};

const EBADF: i32 = 9;
const EACCES: i32 = 13;
const EINVAL: i32 = 22;
const EMFILE: i32 = 24;

/// How the remote encodes `open` flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagEncoding {
    /// newlib (`O_CREAT` = 0x200, ...): bare-metal and most RTOS remotes.
    Newlib,
    /// Linux generic (`O_CREAT` = 0x40, ...): remotes built against Linux
    /// headers, or the C demo's pass-through behaviour.
    Linux,
}

impl FlagEncoding {
    fn normalise(self, f: i32) -> i32 {
        match self {
            FlagEncoding::Newlib => f,
            FlagEncoding::Linux => {
                let mut n = f & O_ACCMODE;
                for (linux, newlib) in [
                    (0o100, O_CREAT),
                    (0o200, O_EXCL),
                    (0o1000, O_TRUNC),
                    (0o2000, O_APPEND),
                ] {
                    if f & linux != 0 {
                        n |= newlib;
                    }
                }
                n
            }
        }
    }
}

/// Executes proxy requests inside a root directory.
pub struct ProxyServer {
    root: PathBuf,
    encoding: FlagEncoding,
    files: HashMap<i32, File>,
    next_fd: i32,
    max_files: usize,
    stdin: Box<dyn Read + Send>,
    stdout: Box<dyn Write + Send>,
    stderr: Box<dyn Write + Send>,
}

impl ProxyServer {
    /// A server confining file access to `root`, using the process's
    /// standard streams for fds 0–2.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ProxyServer {
            root: root.into(),
            encoding: FlagEncoding::Newlib,
            files: HashMap::new(),
            next_fd: 3,
            max_files: 64,
            stdin: Box::new(io::stdin()),
            stdout: Box::new(io::stdout()),
            stderr: Box::new(io::stderr()),
        }
    }

    /// Selects the remote's flag encoding.
    pub fn with_flag_encoding(mut self, e: FlagEncoding) -> Self {
        self.encoding = e;
        self
    }

    /// Replaces the standard streams.
    pub fn with_stdio(
        mut self,
        stdin: Box<dyn Read + Send>,
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
    ) -> Self {
        self.stdin = stdin;
        self.stdout = stdout;
        self.stderr = stderr;
        self
    }

    /// The message telling the remote to shut its proxy down.
    pub fn terminate_message() -> [u8; SYSCALL_SIZE] {
        Syscall {
            id: TERM_SYSCALL_ID,
            int_field1: 0,
            int_field2: 0,
            data_len: 0,
        }
        .to_bytes()
    }

    fn resolve(&self, path: &[u8]) -> Option<PathBuf> {
        let s = core::str::from_utf8(path).ok()?;
        let mut out = self.root.clone();
        for c in Path::new(s).components() {
            match c {
                Component::Normal(p) => out.push(p),
                Component::RootDir | Component::CurDir => {}
                Component::ParentDir | Component::Prefix(_) => return None,
            }
        }
        Some(out)
    }

    fn open(&mut self, path: &[u8], flags: i32) -> i32 {
        let Some(path) = self.resolve(path) else {
            return -EACCES;
        };
        if self.files.len() >= self.max_files {
            return -EMFILE;
        }
        let f = self.encoding.normalise(flags);
        let mut o = OpenOptions::new();
        match f & O_ACCMODE {
            O_RDONLY => o.read(true),
            O_WRONLY => o.write(true),
            O_RDWR => o.read(true).write(true),
            _ => return -EINVAL,
        };
        if f & O_APPEND != 0 {
            o.append(true);
        }
        if f & O_TRUNC != 0 {
            o.truncate(true);
        }
        if f & O_CREAT != 0 {
            if f & O_EXCL != 0 {
                o.create_new(true);
            } else {
                o.create(true);
            }
        }
        match o.open(path) {
            Ok(file) => {
                let fd = self.next_fd;
                self.next_fd += 1;
                self.files.insert(fd, file);
                fd
            }
            Err(e) => -e.raw_os_error().unwrap_or(EINVAL),
        }
    }

    fn write(&mut self, fd: i32, data: &[u8]) -> i32 {
        let r = match fd {
            1 => self
                .stdout
                .write_all(data)
                .and_then(|_| self.stdout.flush()),
            2 => self
                .stderr
                .write_all(data)
                .and_then(|_| self.stderr.flush()),
            fd => match self.files.get_mut(&fd) {
                Some(f) => f.write_all(data),
                None => return -EBADF,
            },
        };
        match r {
            Ok(()) => data.len() as i32,
            Err(e) => -e.raw_os_error().unwrap_or(EINVAL),
        }
    }

    fn read(&mut self, fd: i32, buf: &mut [u8]) -> i32 {
        let r = match fd {
            0 => self.stdin.read(buf),
            fd => match self.files.get_mut(&fd) {
                Some(f) => f.read(buf),
                None => return -EBADF,
            },
        };
        match r {
            Ok(n) => n as i32,
            Err(e) => -e.raw_os_error().unwrap_or(EINVAL),
        }
    }

    /// Handles one request; returns the reply to send, or `None` for a
    /// malformed or unknown request (the C host ignores those too).
    pub fn handle(&mut self, req: &[u8]) -> Option<Vec<u8>> {
        let h = Syscall::from_bytes(req)?;
        let data = req.get(SYSCALL_SIZE..)?;
        let data = &data[..data.len().min(h.data_len as usize)];
        let mut reply = Syscall {
            id: h.id,
            int_field1: 0,
            int_field2: 0,
            data_len: 0,
        };
        let mut payload = Vec::new();
        match h.id {
            OPEN_SYSCALL_ID => {
                let path = data.split(|&c| c == 0).next().unwrap_or(&[]);
                reply.int_field1 = self.open(path, h.int_field1);
            }
            CLOSE_SYSCALL_ID => {
                reply.int_field1 = if self.files.remove(&h.int_field1).is_some() {
                    0
                } else {
                    -EBADF
                };
            }
            WRITE_SYSCALL_ID => {
                let len = (h.int_field2.max(0) as usize).min(data.len());
                reply.int_field1 = self.write(h.int_field1, &data[..len]);
            }
            READ_SYSCALL_ID => {
                let want = (h.int_field2.max(0) as usize).min(MAX_BUF_LEN - SYSCALL_SIZE);
                payload.resize(want, 0);
                let n = self.read(h.int_field1, &mut payload);
                payload.truncate(n.max(0) as usize);
                reply.int_field1 = n;
                reply.data_len = payload.len() as u32;
            }
            _ => return None,
        }
        let mut out = reply.to_bytes().to_vec();
        out.extend_from_slice(&payload);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Channel, ProxyClient};
    use std::sync::{Arc, Mutex};

    struct Direct<'a>(&'a mut ProxyServer);
    impl Channel for Direct<'_> {
        type Error = ();
        fn request(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize, ()> {
            let r = self.0.handle(req).ok_or(())?;
            let n = r.len().min(resp.len());
            resp[..n].copy_from_slice(&r[..n]);
            Ok(n)
        }
    }

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(std::format!("rustamp-proxy-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn file_round_trip_in_sandbox() {
        use crate::flags::*;
        let root = tmpdir("files");
        let out = Capture::default();
        let mut srv = ProxyServer::new(&root).with_stdio(
            Box::new(io::empty()),
            Box::new(out.clone()),
            Box::new(io::sink()),
        );
        let mut c = ProxyClient::new(Direct(&mut srv));
        let fd = c
            .open("/data.txt", O_WRONLY | O_CREAT | O_TRUNC, 0o644)
            .unwrap();
        c.write_all(fd, b"remote wrote this").unwrap();
        c.close(fd).unwrap();
        let fd = c.open("data.txt", O_RDONLY, 0).unwrap();
        let mut buf = [0u8; 64];
        let n = c.read(fd, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"remote wrote this");
        c.close(fd).unwrap();
        assert!(matches!(c.close(fd), Err(crate::Error::Remote(-9))));
        assert!(matches!(
            c.open("missing", O_RDONLY, 0),
            Err(crate::Error::Remote(-2))
        ));
        assert!(matches!(
            c.open("../escape", O_WRONLY | O_CREAT, 0),
            Err(crate::Error::Remote(-13))
        ));
        core::fmt::Write::write_fmt(&mut c.stdout(), format_args!("value = {}\n", 42)).unwrap();
        assert_eq!(&out.0.lock().unwrap()[..], b"value = 42\n");
        assert_eq!(
            std::fs::read(root.join("data.txt")).unwrap(),
            b"remote wrote this"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn linux_flag_encoding() {
        assert_eq!(
            FlagEncoding::Linux.normalise(0o1 | 0o100 | 0o1000),
            O_WRONLY | O_CREAT | O_TRUNC
        );
        assert_eq!(FlagEncoding::Newlib.normalise(0x209), 0x209);
    }

    #[test]
    fn malformed_requests_are_ignored() {
        let mut srv = ProxyServer::new(tmpdir("bad"));
        assert!(srv.handle(&[1, 2, 3]).is_none());
        assert!(srv.handle(&ProxyServer::terminate_message()).is_none());
    }
}
