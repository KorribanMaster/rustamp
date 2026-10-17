// SPDX-License-Identifier: BSD-3-Clause
//! Linux userspace backend (`lib/system/linux`).
//!
//! * [`UioDevice`]: memory maps and interrupts of a device bound to a UIO
//!   driver (`uio_pdrv_genirq`, `uio_pci_generic`, ...).
//! * [`map_shared_file`]: a file in `/dev/shm` (or any file) mapped shared,
//!   for host-to-host testing between processes.
//!
//! Interrupt handling follows UIO semantics: [`UioDevice::wait_irq`] blocks
//! in `read(2)` and [`UioDevice::enable_irq`] re-arms the line with
//! `write(2)`. For event loops, poll [`UioDevice::as_raw_fd`].

use std::format;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::string::{String, ToString};
use std::vec::Vec;

use crate::{Error, IoRegion, Result};

/// The handful of libc symbols this backend needs (std already links libc).
mod sys {
    use core::ffi::{c_int, c_long, c_void};

    pub(super) const PROT_READ: c_int = 1;
    pub(super) const PROT_WRITE: c_int = 2;
    pub(super) const MAP_SHARED: c_int = 1;
    pub(super) const MAP_FAILED: *mut c_void = !0usize as *mut c_void;
    pub(super) const SC_PAGESIZE: c_int = 30;
    pub(super) const ENOENT: i32 = 2;

    unsafe extern "C" {
        pub(super) fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            off: i64,
        ) -> *mut c_void;
        pub(super) fn munmap(addr: *mut c_void, len: usize) -> c_int;
        pub(super) fn sysconf(name: c_int) -> c_long;
    }
}

fn last_os_error() -> Error {
    Error::Os(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
}

fn io_err(e: std::io::Error) -> Error {
    Error::Os(e.raw_os_error().unwrap_or(0))
}

fn page_size() -> usize {
    // SAFETY: sysconf has no preconditions.
    let p = unsafe { sys::sysconf(sys::SC_PAGESIZE) };
    if p > 0 { p as usize } else { 4096 }
}

/// An `mmap`ed range, unmapped on drop.
#[derive(Debug)]
pub struct Mapping {
    base: *mut u8,
    map_len: usize,
    start: usize,
    len: usize,
}

// SAFETY: the mapping is plain shared memory; access goes through IoRegion.
unsafe impl Send for Mapping {}
// SAFETY: as above.
unsafe impl Sync for Mapping {}

impl Mapping {
    fn new(fd: RawFd, len: usize, file_offset: i64, start: usize) -> Result<Self> {
        let map_len = crate::align_up(start + len, page_size());
        // SAFETY: a fresh shared mapping of a file descriptor we own; the
        // kernel validates the arguments.
        let p = unsafe {
            sys::mmap(
                core::ptr::null_mut(),
                map_len,
                sys::PROT_READ | sys::PROT_WRITE,
                sys::MAP_SHARED,
                fd,
                file_offset,
            )
        };
        if p == sys::MAP_FAILED {
            return Err(last_os_error());
        }
        Ok(Mapping {
            base: p.cast(),
            map_len,
            start,
            len,
        })
    }

    /// Length of the usable part of the mapping.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the mapping is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// A region over the mapping, seen by the peer at `phys`.
    ///
    /// # Safety
    ///
    /// The region (and its clones) must not be used after the mapping is
    /// dropped. Use [`leak`](Self::leak) when the mapping lives for the whole
    /// program.
    pub unsafe fn region(&self, phys: u64) -> IoRegion {
        // SAFETY: the mapping is valid for `len` bytes from `start`; the
        // lifetime obligation is passed to the caller.
        unsafe { IoRegion::new(self.base.add(self.start), phys, self.len) }
    }

    /// Keeps the mapping forever and returns a region over it.
    pub fn leak(self, phys: u64) -> IoRegion {
        // SAFETY: the mapping is never unmapped after `forget`.
        let r = unsafe { self.region(phys) };
        core::mem::forget(self);
        r
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly what `mmap` returned.
        unsafe { sys::munmap(self.base.cast(), self.map_len) };
    }
}

/// Maps `size` bytes of the file at `path` shared, creating and sizing it if
/// needed. Both processes of a host-side test map the same file.
pub fn map_shared_file(path: impl AsRef<Path>, size: usize) -> Result<Mapping> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io_err)?;
    let cur = f.metadata().map_err(io_err)?.len();
    if cur < size as u64 {
        f.set_len(size as u64).map_err(io_err)?;
    }
    Mapping::new(f.as_raw_fd(), size, 0, 0)
}

/// One memory map of a UIO device, from `/sys/class/uio/uioN/maps/mapM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UioMap {
    /// Physical address of the map.
    pub addr: u64,
    /// Size in bytes.
    pub size: usize,
    /// Offset of `addr` within its first page.
    pub offset: usize,
}

/// A device bound to a UIO driver.
#[derive(Debug)]
pub struct UioDevice {
    file: File,
    name: String,
    maps: Vec<UioMap>,
}

fn read_sysfs(p: &Path) -> Result<String> {
    Ok(fs::read_to_string(p).map_err(io_err)?.trim().to_string())
}

fn parse_num(s: &str) -> Result<u64> {
    let s = s.trim();
    let r = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|_| Error::InvalidParam)
}

impl UioDevice {
    /// Opens `/dev/uioN` and reads its maps from sysfs.
    pub fn open(dev: impl AsRef<Path>) -> Result<Self> {
        let dev = dev.as_ref();
        let uio = dev.file_name().ok_or(Error::InvalidParam)?;
        Self::open_with_sysfs(dev, &Path::new("/sys/class/uio").join(uio))
    }

    /// Finds the UIO device whose `name` attribute equals `name` (the
    /// device-tree node name for `uio_pdrv_genirq`) and opens it.
    pub fn open_by_name(name: &str) -> Result<Self> {
        for e in fs::read_dir("/sys/class/uio").map_err(io_err)? {
            let e = e.map_err(io_err)?;
            if read_sysfs(&e.path().join("name")).ok().as_deref() == Some(name) {
                return Self::open(Path::new("/dev").join(e.file_name()));
            }
        }
        Err(Error::Os(sys::ENOENT))
    }

    fn open_with_sysfs(dev: &Path, sysfs: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dev)
            .map_err(io_err)?;
        let name = read_sysfs(&sysfs.join("name")).unwrap_or_default();
        let mut maps = Vec::new();
        for i in 0.. {
            let m: PathBuf = sysfs.join(format!("maps/map{i}"));
            if !m.exists() {
                break;
            }
            maps.push(UioMap {
                addr: parse_num(&read_sysfs(&m.join("addr"))?)?,
                size: parse_num(&read_sysfs(&m.join("size"))?)? as usize,
                offset: read_sysfs(&m.join("offset"))
                    .and_then(|s| parse_num(&s))
                    .unwrap_or(0) as usize,
            });
        }
        Ok(UioDevice { file, name, maps })
    }

    /// The device's `name` attribute.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The device's memory maps.
    pub fn maps(&self) -> &[UioMap] {
        &self.maps
    }

    /// Maps memory map `index`. The returned mapping's region starts at the
    /// map's physical address (UIO's in-page `offset` is applied).
    pub fn map(&self, index: usize) -> Result<(Mapping, UioMap)> {
        let m = *self.maps.get(index).ok_or(Error::InvalidParam)?;
        // UIO selects map N with an mmap offset of N pages.
        let off = (index * page_size()) as i64;
        let mapping = Mapping::new(self.file.as_raw_fd(), m.size, off, m.offset)?;
        Ok((mapping, m))
    }

    /// Re-enables the interrupt (writes `1` to the device).
    pub fn enable_irq(&self) -> Result<()> {
        (&self.file).write_all(&1u32.to_ne_bytes()).map_err(io_err)
    }

    /// Blocks until the next interrupt; returns the kernel's event count.
    pub fn wait_irq(&self) -> Result<u32> {
        let mut b = [0u8; 4];
        (&self.file).read_exact(&mut b).map_err(io_err)?;
        Ok(u32::from_ne_bytes(b))
    }

    /// The file descriptor, for `poll`/`epoll`.
    pub fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

/// Unlinks a shared-memory file created with [`map_shared_file`].
pub fn remove_shared_file(path: impl AsRef<Path>) -> Result<()> {
    fs::remove_file(path).map_err(io_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Miri cannot mmap files")]
    fn shared_file_is_shared_between_mappings() {
        let path = std::env::temp_dir().join(format!("openamp-metal-test-{}", std::process::id()));
        let a = map_shared_file(&path, 8192).unwrap().leak(0x1000_0000);
        let b = map_shared_file(&path, 8192).unwrap().leak(0x1000_0000);
        a.write_u32(4096, 0x1234_5678).unwrap();
        assert_eq!(b.read_u32(4096), Ok(0x1234_5678));
        remove_shared_file(&path).unwrap();
    }

    #[test]
    fn sysfs_number_parsing() {
        assert_eq!(parse_num("0x4000"), Ok(0x4000));
        assert_eq!(parse_num("4096\n"), Ok(4096));
        assert!(parse_num("zz").is_err());
    }

    #[test]
    fn uio_from_fake_sysfs() {
        let dir = std::env::temp_dir().join(format!("openamp-uio-{}", std::process::id()));
        let sys = dir.join("uio7");
        fs::create_dir_all(sys.join("maps/map0")).unwrap();
        fs::write(sys.join("name"), "vring-mem\n").unwrap();
        fs::write(sys.join("maps/map0/addr"), "0x55000000\n").unwrap();
        fs::write(sys.join("maps/map0/size"), "0x8000\n").unwrap();
        fs::write(sys.join("maps/map0/offset"), "0x0\n").unwrap();
        let dev = dir.join("dev-uio7");
        fs::write(&dev, []).unwrap();
        let d = UioDevice::open_with_sysfs(&dev, &sys).unwrap();
        assert_eq!(d.name(), "vring-mem");
        assert_eq!(
            d.maps(),
            &[UioMap {
                addr: 0x5500_0000,
                size: 0x8000,
                offset: 0
            }]
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
