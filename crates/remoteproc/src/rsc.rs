// SPDX-License-Identifier: BSD-3-Clause
//! Resource tables (`remoteproc.h`, `rsc_table_parser.c`).
//!
//! Two halves:
//!
//! * Firmware-side `#[repr(C)]` types ([`RscTableHeader`], [`FwRscCarveout`],
//!   [`FwRscVdev`], ...) with `const` constructors, used through the
//!   [`resource_table!`](crate::resource_table) macro.
//! * A bounds-checked parser over an [`IoRegion`] ([`ResourceTable`],
//!   [`VdevRsc`]) used by hosts and by firmware at run time. The host writes
//!   into the firmware's table (status, negotiated features, vring
//!   addresses), so the table is only ever accessed through the region.

use core::cell::UnsafeCell;
use core::sync::atomic::Ordering;

use openamp_metal::IoRegion;

use crate::{Error, Result};

/// Resource type: physically contiguous memory for the remote.
pub const RSC_CARVEOUT: u32 = 0;
/// Resource type: device memory (MMIO) the remote needs mapped.
pub const RSC_DEVMEM: u32 = 1;
/// Resource type: a trace buffer the host can read.
pub const RSC_TRACE: u32 = 2;
/// Resource type: a virtio device.
pub const RSC_VDEV: u32 = 3;
/// First vendor-specific resource type.
pub const RSC_VENDOR_START: u32 = 128;
/// Last vendor-specific resource type.
pub const RSC_VENDOR_END: u32 = 512;
/// Address "to be allocated by the host" (`FW_RSC_U32_ADDR_ANY`).
pub const FW_RSC_ADDR_ANY: u32 = 0xFFFF_FFFF;
/// Notify id "to be assigned by the host" (`RSC_NOTIFY_ID_ANY`).
pub const RSC_NOTIFY_ID_ANY: u32 = 0xFFFF_FFFF;
/// Resource table version understood by Linux and OpenAMP.
pub const RSC_TABLE_VERSION: u32 = 1;

const fn name32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    assert!(b.len() < 32, "resource name must be shorter than 32 bytes");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < b.len() {
        out[i] = b[i];
        i += 1;
    }
    out
}

/// Resource table header with `N` entries (`struct resource_table`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RscTableHeader<const N: usize> {
    /// Version, always 1.
    pub ver: u32,
    /// Number of entries.
    pub num: u32,
    /// Reserved, zero.
    pub reserved: [u32; 2],
    /// Byte offset of each entry from the start of the table.
    pub offset: [u32; N],
}

impl<const N: usize> RscTableHeader<N> {
    /// A header for entries at `offset`.
    pub const fn new(offset: [u32; N]) -> Self {
        RscTableHeader {
            ver: RSC_TABLE_VERSION,
            num: N as u32,
            reserved: [0; 2],
            offset,
        }
    }
}

/// `struct fw_rsc_carveout` (also used for devmem).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FwRscCarveout {
    /// [`RSC_CARVEOUT`] or [`RSC_DEVMEM`].
    pub rsc_type: u32,
    /// Device address.
    pub da: u32,
    /// Physical address.
    pub pa: u32,
    /// Length in bytes.
    pub len: u32,
    /// IOMMU protection flags.
    pub flags: u32,
    /// Reserved.
    pub reserved: u32,
    /// Name.
    pub name: [u8; 32],
}

impl FwRscCarveout {
    /// A carve-out entry.
    pub const fn new(name: &str, da: u32, pa: u32, len: u32, flags: u32) -> Self {
        FwRscCarveout {
            rsc_type: RSC_CARVEOUT,
            da,
            pa,
            len,
            flags,
            reserved: 0,
            name: name32(name),
        }
    }

    /// A devmem entry (same layout).
    pub const fn devmem(name: &str, da: u32, pa: u32, len: u32, flags: u32) -> Self {
        let mut c = Self::new(name, da, pa, len, flags);
        c.rsc_type = RSC_DEVMEM;
        c
    }
}

/// `struct fw_rsc_trace`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FwRscTrace {
    /// [`RSC_TRACE`].
    pub rsc_type: u32,
    /// Device address of the trace buffer.
    pub da: u32,
    /// Length.
    pub len: u32,
    /// Reserved.
    pub reserved: u32,
    /// Name.
    pub name: [u8; 32],
}

impl FwRscTrace {
    /// A trace-buffer entry.
    pub const fn new(name: &str, da: u32, len: u32) -> Self {
        FwRscTrace {
            rsc_type: RSC_TRACE,
            da,
            len,
            reserved: 0,
            name: name32(name),
        }
    }
}

/// `struct fw_rsc_vdev_vring`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FwRscVdevVring {
    /// Device address of the ring, or [`FW_RSC_ADDR_ANY`].
    pub da: u32,
    /// Used-ring alignment.
    pub align: u32,
    /// Number of entries.
    pub num: u32,
    /// Notify id.
    pub notifyid: u32,
    /// Reserved (physical address in some trees).
    pub pa: u32,
}

impl FwRscVdevVring {
    /// A vring entry.
    pub const fn new(da: u32, align: u32, num: u32, notifyid: u32) -> Self {
        FwRscVdevVring {
            da,
            align,
            num,
            notifyid,
            pa: 0,
        }
    }
}

/// `struct fw_rsc_vdev` with `V` vrings and no config space.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FwRscVdev<const V: usize> {
    /// [`RSC_VDEV`].
    pub rsc_type: u32,
    /// Virtio device id.
    pub id: u32,
    /// Notify id for status/feature changes.
    pub notifyid: u32,
    /// Features offered by the firmware.
    pub dfeatures: u32,
    /// Features accepted by the host (written by the host).
    pub gfeatures: u32,
    /// Size of the config space following the vrings.
    pub config_len: u32,
    /// Virtio status (written by the host).
    pub status: u8,
    /// Number of vrings.
    pub num_of_vrings: u8,
    /// Reserved.
    pub reserved: [u8; 2],
    /// The vrings.
    pub vring: [FwRscVdevVring; V],
}

impl<const V: usize> FwRscVdev<V> {
    /// A vdev entry (`id` 7 for rpmsg; `dfeatures` bit 0 = name service).
    pub const fn new(id: u32, notifyid: u32, dfeatures: u32, vring: [FwRscVdevVring; V]) -> Self {
        assert!(V <= 255);
        FwRscVdev {
            rsc_type: RSC_VDEV,
            id,
            notifyid,
            dfeatures,
            gfeatures: 0,
            config_len: 0,
            status: 0,
            num_of_vrings: V as u8,
            reserved: [0; 2],
            vring,
        }
    }
}

const _: () = {
    use core::mem::size_of;
    assert!(size_of::<RscTableHeader<2>>() == 24);
    assert!(size_of::<FwRscCarveout>() == 56);
    assert!(size_of::<FwRscTrace>() == 48);
    assert!(size_of::<FwRscVdevVring>() == 20);
    assert!(size_of::<FwRscVdev<2>>() == 68);
};

/// A resource table placed in a `static` and shared with the host.
///
/// The host rewrites parts of the table at run time, so after start-up the
/// firmware accesses it only through [`io_region`](Self::io_region).
#[repr(transparent)]
pub struct SharedTable<T>(UnsafeCell<T>);

// SAFETY: the contents are only accessed through IoRegion (volatile/atomic)
// after construction; no Rust references to the inner value are handed out.
unsafe impl<T> Sync for SharedTable<T> {}

impl<T> SharedTable<T> {
    /// Wraps a table.
    pub const fn new(table: T) -> Self {
        SharedTable(UnsafeCell::new(table))
    }

    /// Address of the table.
    pub fn as_ptr(&self) -> *mut T {
        self.0.get()
    }

    /// Size of the table in bytes.
    pub const fn size(&self) -> usize {
        core::mem::size_of::<T>()
    }

    /// A region over the table, whose device address is its own address
    /// (identity-mapped firmware).
    pub fn io_region(&'static self) -> IoRegion {
        self.io_region_at(self.as_ptr() as usize as u64)
    }

    /// A region over the table seen by the host at device address `da`.
    pub fn io_region_at(&'static self, da: u64) -> IoRegion {
        // SAFETY: a `'static` allocation of `size_of::<T>()` bytes, accessed
        // only through regions from here on.
        unsafe { IoRegion::new(self.as_ptr().cast(), da, self.size()) }
    }
}

/// The type of a resource entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RscType {
    /// [`RSC_CARVEOUT`].
    Carveout,
    /// [`RSC_DEVMEM`].
    Devmem,
    /// [`RSC_TRACE`].
    Trace,
    /// [`RSC_VDEV`].
    Vdev,
    /// A vendor type (128..=512).
    Vendor(u32),
    /// Anything else (rejected by Linux).
    Unknown(u32),
}

impl From<u32> for RscType {
    fn from(t: u32) -> Self {
        match t {
            RSC_CARVEOUT => RscType::Carveout,
            RSC_DEVMEM => RscType::Devmem,
            RSC_TRACE => RscType::Trace,
            RSC_VDEV => RscType::Vdev,
            RSC_VENDOR_START..=RSC_VENDOR_END => RscType::Vendor(t),
            t => RscType::Unknown(t),
        }
    }
}

/// One entry of a parsed table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Index in the offset array.
    pub index: u32,
    /// Offset of the entry within the table's region.
    pub offset: usize,
    /// Entry type.
    pub rsc_type: RscType,
    /// Bytes from the entry to the end of the table.
    pub max_len: usize,
}

/// A carve-out or devmem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carveout {
    /// Device address.
    pub da: u32,
    /// Physical address.
    pub pa: u32,
    /// Length.
    pub len: u32,
    /// Flags.
    pub flags: u32,
    /// Name (NUL-padded).
    pub name: [u8; 32],
}

/// A trace-buffer entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trace {
    /// Device address.
    pub da: u32,
    /// Length.
    pub len: u32,
    /// Name (NUL-padded).
    pub name: [u8; 32],
}

/// A parsed, bounds-checked view of a resource table in shared memory.
#[derive(Debug, Clone)]
pub struct ResourceTable {
    io: IoRegion,
    base: usize,
    len: usize,
    num: u32,
}

impl ResourceTable {
    /// Validates the table at `base..base + len` of `io`.
    pub fn new(io: IoRegion, base: usize, len: usize) -> Result<Self> {
        io.check(base, len)?;
        if len < 16 {
            return Err(Error::InvalidTable);
        }
        let ver = io.read_u32(base)?;
        let num = io.read_u32(base + 4)?;
        if ver != RSC_TABLE_VERSION {
            return Err(Error::InvalidTable);
        }
        let hdr_len = (num as usize)
            .checked_mul(4)
            .and_then(|n| n.checked_add(16))
            .ok_or(Error::InvalidTable)?;
        if hdr_len > len {
            return Err(Error::InvalidTable);
        }
        let t = ResourceTable { io, base, len, num };
        for i in 0..num {
            let off = t.raw_offset(i)?;
            if off < hdr_len || off.checked_add(4).is_none_or(|e| e > len) || off % 4 != 0 {
                return Err(Error::InvalidTable);
            }
        }
        Ok(t)
    }

    /// The region holding the table.
    pub fn io(&self) -> &IoRegion {
        &self.io
    }

    /// Offset of the table within its region.
    pub fn base(&self) -> usize {
        self.base
    }

    /// Table size in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the table has no entries.
    pub fn is_empty(&self) -> bool {
        self.num == 0
    }

    /// Number of entries.
    pub fn num(&self) -> u32 {
        self.num
    }

    fn raw_offset(&self, i: u32) -> Result<usize> {
        Ok(self.io.read_u32(self.base + 16 + 4 * i as usize)? as usize)
    }

    /// Entry `i`.
    pub fn entry(&self, i: u32) -> Result<Entry> {
        if i >= self.num {
            return Err(Error::InvalidTable);
        }
        let off = self.raw_offset(i)?;
        let t = self.io.read_u32(self.base + off)?;
        Ok(Entry {
            index: i,
            offset: self.base + off,
            rsc_type: RscType::from(t),
            max_len: self.len - off,
        })
    }

    /// All entries in order.
    pub fn entries(&self) -> impl Iterator<Item = Result<Entry>> + '_ {
        (0..self.num).map(move |i| self.entry(i))
    }

    fn need(&self, e: &Entry, size: usize) -> Result<()> {
        if e.max_len < size {
            Err(Error::InvalidTable)
        } else {
            Ok(())
        }
    }

    fn read_name(&self, off: usize) -> Result<[u8; 32]> {
        let mut n = [0u8; 32];
        self.io.read_bytes(off, &mut n)?;
        Ok(n)
    }

    /// Decodes a carve-out or devmem entry.
    pub fn carveout(&self, e: &Entry) -> Result<Carveout> {
        if !matches!(e.rsc_type, RscType::Carveout | RscType::Devmem) {
            return Err(Error::InvalidTable);
        }
        self.need(e, 56)?;
        let r = |o: usize| self.io.read_u32(e.offset + o);
        Ok(Carveout {
            da: r(4)?,
            pa: r(8)?,
            len: r(12)?,
            flags: r(16)?,
            name: self.read_name(e.offset + 24)?,
        })
    }

    /// Decodes a trace entry.
    pub fn trace(&self, e: &Entry) -> Result<Trace> {
        if e.rsc_type != RscType::Trace {
            return Err(Error::InvalidTable);
        }
        self.need(e, 48)?;
        Ok(Trace {
            da: self.io.read_u32(e.offset + 4)?,
            len: self.io.read_u32(e.offset + 8)?,
            name: self.read_name(e.offset + 16)?,
        })
    }

    /// A view of a vdev entry.
    pub fn vdev(&self, e: &Entry) -> Result<VdevRsc> {
        if e.rsc_type != RscType::Vdev {
            return Err(Error::InvalidTable);
        }
        self.need(e, VDEV_HDR)?;
        let num_vrings = self.io.read_u8(e.offset + 25)?;
        let config_len = self.io.read_u32(e.offset + 20)?;
        let size = VDEV_HDR + VRING_SIZE * num_vrings as usize;
        self.need(e, size.checked_add(config_len as usize).ok_or(Error::InvalidTable)?)?;
        Ok(VdevRsc {
            io: self.io.clone(),
            off: e.offset,
            num_vrings,
            config_len,
        })
    }

    /// The `n`th vdev entry.
    pub fn find_vdev(&self, n: usize) -> Result<VdevRsc> {
        let e = self
            .entries()
            .filter(|e| matches!(e, Ok(e) if e.rsc_type == RscType::Vdev) || e.is_err())
            .nth(n)
            .ok_or(Error::NoResourceTable)??;
        self.vdev(&e)
    }
}

const VDEV_HDR: usize = 28;
const VRING_SIZE: usize = 20;

/// A vring entry as read from a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VringRsc {
    /// Device address, or [`FW_RSC_ADDR_ANY`].
    pub da: u32,
    /// Used-ring alignment.
    pub align: u32,
    /// Number of entries.
    pub num: u32,
    /// Notify id.
    pub notifyid: u32,
}

/// A live view of a vdev entry in a resource table.
#[derive(Debug, Clone)]
pub struct VdevRsc {
    io: IoRegion,
    off: usize,
    num_vrings: u8,
    config_len: u32,
}

impl VdevRsc {
    /// Virtio device id.
    pub fn id(&self) -> u32 {
        self.io.read_u32(self.off + 4).unwrap_or(0)
    }

    /// Notify id of the vdev.
    pub fn notifyid(&self) -> u32 {
        self.io.read_u32(self.off + 8).unwrap_or(0)
    }

    /// Sets the vdev notify id (host).
    pub fn set_notifyid(&self, id: u32) -> Result<()> {
        Ok(self.io.write_u32(self.off + 8, id)?)
    }

    /// Features offered by the firmware.
    pub fn dfeatures(&self) -> u32 {
        self.io.read_u32(self.off + 12).unwrap_or(0)
    }

    /// Features accepted by the host.
    pub fn gfeatures(&self) -> u32 {
        self.io.read_u32(self.off + 16).unwrap_or(0)
    }

    /// Writes the accepted features (host).
    pub fn set_gfeatures(&self, f: u32) -> Result<()> {
        Ok(self.io.write_u32(self.off + 16, f)?)
    }

    /// Virtio status.
    pub fn status(&self) -> u8 {
        self.io.load_u8(self.off + 24, Ordering::Acquire).unwrap_or(0)
    }

    /// Writes the virtio status.
    pub fn set_status(&self, s: u8) -> Result<()> {
        Ok(self.io.store_u8(self.off + 24, s, Ordering::Release)?)
    }

    /// Number of vrings.
    pub fn num_vrings(&self) -> usize {
        self.num_vrings as usize
    }

    fn vring_off(&self, i: usize) -> Result<usize> {
        if i >= self.num_vrings as usize {
            return Err(Error::InvalidTable);
        }
        Ok(self.off + VDEV_HDR + VRING_SIZE * i)
    }

    /// Vring `i`.
    pub fn vring(&self, i: usize) -> Result<VringRsc> {
        let o = self.vring_off(i)?;
        Ok(VringRsc {
            da: self.io.read_u32(o)?,
            align: self.io.read_u32(o + 4)?,
            num: self.io.read_u32(o + 8)?,
            notifyid: self.io.read_u32(o + 12)?,
        })
    }

    /// Writes vring `i`'s device address (host, after allocating it).
    pub fn set_vring_da(&self, i: usize, da: u32) -> Result<()> {
        Ok(self.io.write_u32(self.vring_off(i)?, da)?)
    }

    /// Writes vring `i`'s notify id (host).
    pub fn set_vring_notifyid(&self, i: usize, id: u32) -> Result<()> {
        Ok(self.io.write_u32(self.vring_off(i)? + 12, id)?)
    }

    /// Size of the config space.
    pub fn config_len(&self) -> u32 {
        self.config_len
    }

    /// Offset of the config space within the region.
    pub fn config_offset(&self) -> usize {
        self.off + VDEV_HDR + VRING_SIZE * self.num_vrings as usize
    }

    /// The region holding the table.
    pub fn io(&self) -> &IoRegion {
        &self.io
    }

    /// Offset of this entry within the region.
    pub fn offset(&self) -> usize {
        self.off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::resource_table! {
        static TABLE: TestTable = {
            mem: FwRscCarveout = FwRscCarveout::new("fw-mem", 0x1000, 0x2000, 0x8000, 0),
            trace: FwRscTrace = FwRscTrace::new("trace0", 0x3000, 0x400),
            vdev: FwRscVdev<2> = FwRscVdev::new(7, 9, 1, [
                FwRscVdevVring::new(FW_RSC_ADDR_ANY, 4096, 16, 0),
                FwRscVdevVring::new(0x5500_8000, 4096, 16, 1),
            ]),
        };
    }

    #[test]
    fn macro_layout_parses() {
        let io = TABLE.io_region_at(0x1ffe_0000);
        let t = ResourceTable::new(io.clone(), 0, TABLE.size()).unwrap();
        assert_eq!(t.num(), 3);
        let kinds: std::vec::Vec<_> = t.entries().map(|e| e.unwrap().rsc_type).collect();
        assert_eq!(kinds, [RscType::Carveout, RscType::Trace, RscType::Vdev]);
        let c = t.carveout(&t.entry(0).unwrap()).unwrap();
        assert_eq!((c.da, c.pa, c.len), (0x1000, 0x2000, 0x8000));
        assert_eq!(&c.name[..7], b"fw-mem\0");
        assert_eq!(t.trace(&t.entry(1).unwrap()).unwrap().len, 0x400);
        let v = t.find_vdev(0).unwrap();
        assert_eq!((v.id(), v.notifyid(), v.dfeatures()), (7, 9, 1));
        assert_eq!(v.num_vrings(), 2);
        assert_eq!(v.vring(0).unwrap().da, FW_RSC_ADDR_ANY);
        assert_eq!(v.vring(1).unwrap().da, 0x5500_8000);
        v.set_status(4).unwrap();
        assert_eq!(v.status(), 4);
        assert!(v.vring(2).is_err());
        assert!(t.find_vdev(1).is_err());
    }

    #[test]
    fn malformed_tables_are_rejected() {
        let io = IoRegion::alloc(256, 0);
        assert_eq!(ResourceTable::new(io.clone(), 0, 64).unwrap_err(), Error::InvalidTable); // ver 0
        io.write_u32(0, 1).unwrap();
        io.write_u32(4, 100).unwrap(); // too many entries
        assert_eq!(ResourceTable::new(io.clone(), 0, 64).unwrap_err(), Error::InvalidTable);
        io.write_u32(4, 1).unwrap();
        io.write_u32(16, 64).unwrap(); // entry offset past the end
        assert_eq!(ResourceTable::new(io.clone(), 0, 64).unwrap_err(), Error::InvalidTable);
        io.write_u32(16, 24).unwrap();
        io.write_u32(24, RSC_VDEV).unwrap();
        io.write_u8(24 + 25, 200).unwrap(); // 200 vrings cannot fit
        let t = ResourceTable::new(io.clone(), 0, 64).unwrap();
        assert_eq!(t.find_vdev(0).unwrap_err(), Error::InvalidTable);
    }
}
