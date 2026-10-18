// SPDX-License-Identifier: BSD-3-Clause
//! A streaming ELF loader (`elf_loader.c`).
//!
//! Reads headers and segments on demand through an [`ImageStore`], so a
//! host with little RAM can load a large image from flash or a file. Both
//! ELF32 and ELF64 little-endian images are supported; segments are placed
//! at their physical address (`p_paddr`), as Linux remoteproc does.

use openamp_metal::IoRegion;

use crate::{Error, Result};

/// Random-access source of a firmware image (`image_store_ops`).
pub trait ImageStore {
    /// Fills `buf` from image offset `offset`, failing if the image is too
    /// short.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()>;
}

impl ImageStore for &[u8] {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).map_err(|_| Error::InvalidImage)?;
        let end = start.checked_add(buf.len()).ok_or(Error::InvalidImage)?;
        let src = self.get(start..end).ok_or(Error::InvalidImage)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

#[cfg(feature = "std")]
impl ImageStore for std::fs::File {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        use std::os::unix::fs::FileExt;
        self.read_exact_at(buf, offset)
            .map_err(|_| Error::InvalidImage)
    }
}

/// ELF class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// 32-bit.
    Elf32,
    /// 64-bit.
    Elf64,
}

/// Program header type of loadable segments.
pub const PT_LOAD: u32 = 1;

/// The ELF file header fields the loader needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// 32 or 64 bit.
    pub class: Class,
    /// `e_machine`.
    pub machine: u16,
    /// Entry point.
    pub entry: u64,
    phoff: u64,
    phentsize: u16,
    /// Number of program headers.
    pub phnum: u16,
    shoff: u64,
    shentsize: u16,
    /// Number of section headers.
    pub shnum: u16,
    shstrndx: u16,
}

/// A program header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// `p_type`.
    pub p_type: u32,
    /// Offset in the image.
    pub offset: u64,
    /// Virtual address.
    pub vaddr: u64,
    /// Physical (load) address.
    pub paddr: u64,
    /// Bytes in the image.
    pub filesz: u64,
    /// Bytes in memory (the rest is zeroed).
    pub memsz: u64,
}

/// A section header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    /// Address.
    pub addr: u64,
    /// Offset in the image.
    pub offset: u64,
    /// Size.
    pub size: u64,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// Reads and validates the file header.
pub fn header(img: &mut impl ImageStore) -> Result<Header> {
    let mut b = [0u8; 64];
    img.read_at(0, &mut b[..52])?;
    if b[..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(Error::InvalidImage);
    }
    if b[5] != 1 {
        return Err(Error::UnsupportedImage); // big-endian
    }
    if b[6] != 1 {
        return Err(Error::InvalidImage);
    }
    let e_type = u16_at(&b, 16);
    if e_type != 2 && e_type != 3 {
        return Err(Error::UnsupportedImage); // not EXEC or DYN
    }
    let h = match b[4] {
        1 => Header {
            class: Class::Elf32,
            machine: u16_at(&b, 18),
            entry: u32_at(&b, 24) as u64,
            phoff: u32_at(&b, 28) as u64,
            shoff: u32_at(&b, 32) as u64,
            phentsize: u16_at(&b, 42),
            phnum: u16_at(&b, 44),
            shentsize: u16_at(&b, 46),
            shnum: u16_at(&b, 48),
            shstrndx: u16_at(&b, 50),
        },
        2 => {
            img.read_at(0, &mut b)?;
            Header {
                class: Class::Elf64,
                machine: u16_at(&b, 18),
                entry: u64_at(&b, 24),
                phoff: u64_at(&b, 32),
                shoff: u64_at(&b, 40),
                phentsize: u16_at(&b, 54),
                phnum: u16_at(&b, 56),
                shentsize: u16_at(&b, 58),
                shnum: u16_at(&b, 60),
                shstrndx: u16_at(&b, 62),
            }
        }
        _ => return Err(Error::InvalidImage),
    };
    let (ph, sh) = match h.class {
        Class::Elf32 => (32, 40),
        Class::Elf64 => (56, 64),
    };
    if (h.phnum > 0 && (h.phentsize as usize) < ph) || (h.shnum > 0 && (h.shentsize as usize) < sh)
    {
        return Err(Error::InvalidImage);
    }
    Ok(h)
}

/// Program header `i`.
pub fn segment(img: &mut impl ImageStore, h: &Header, i: u16) -> Result<Segment> {
    if i >= h.phnum {
        return Err(Error::InvalidImage);
    }
    let off = h
        .phoff
        .checked_add(i as u64 * h.phentsize as u64)
        .ok_or(Error::InvalidImage)?;
    let mut b = [0u8; 56];
    Ok(match h.class {
        Class::Elf32 => {
            img.read_at(off, &mut b[..32])?;
            Segment {
                p_type: u32_at(&b, 0),
                offset: u32_at(&b, 4) as u64,
                vaddr: u32_at(&b, 8) as u64,
                paddr: u32_at(&b, 12) as u64,
                filesz: u32_at(&b, 16) as u64,
                memsz: u32_at(&b, 20) as u64,
            }
        }
        Class::Elf64 => {
            img.read_at(off, &mut b)?;
            Segment {
                p_type: u32_at(&b, 0),
                offset: u64_at(&b, 8),
                vaddr: u64_at(&b, 16),
                paddr: u64_at(&b, 24),
                filesz: u64_at(&b, 32),
                memsz: u64_at(&b, 40),
            }
        }
    })
}

fn section_header(img: &mut impl ImageStore, h: &Header, i: u16) -> Result<(u32, Section)> {
    if i >= h.shnum {
        return Err(Error::InvalidImage);
    }
    let off = h
        .shoff
        .checked_add(i as u64 * h.shentsize as u64)
        .ok_or(Error::InvalidImage)?;
    let mut b = [0u8; 64];
    Ok(match h.class {
        Class::Elf32 => {
            img.read_at(off, &mut b[..40])?;
            (
                u32_at(&b, 0),
                Section {
                    addr: u32_at(&b, 12) as u64,
                    offset: u32_at(&b, 16) as u64,
                    size: u32_at(&b, 20) as u64,
                },
            )
        }
        Class::Elf64 => {
            img.read_at(off, &mut b)?;
            (
                u32_at(&b, 0),
                Section {
                    addr: u64_at(&b, 16),
                    offset: u64_at(&b, 24),
                    size: u64_at(&b, 32),
                },
            )
        }
    })
}

/// Finds the section called `name` (e.g. `b".resource_table"`).
pub fn find_section(img: &mut impl ImageStore, h: &Header, name: &[u8]) -> Result<Option<Section>> {
    if h.shnum == 0 {
        return Ok(None);
    }
    let (_, strtab) = section_header(img, h, h.shstrndx)?;
    let mut buf = [0u8; 64];
    let n = name.len() + 1;
    if n > buf.len() {
        return Err(Error::InvalidImage);
    }
    for i in 0..h.shnum {
        let (name_off, s) = section_header(img, h, i)?;
        if name_off as u64 + n as u64 > strtab.size {
            continue;
        }
        let at = strtab
            .offset
            .checked_add(name_off as u64)
            .ok_or(Error::InvalidImage)?;
        img.read_at(at, &mut buf[..n])?;
        if &buf[..n - 1] == name && buf[n - 1] == 0 {
            return Ok(Some(s));
        }
    }
    Ok(None)
}

/// Copies every `PT_LOAD` segment to memory obtained from `map(paddr,
/// memsz)` and zeroes the part beyond `filesz`.
pub fn load_segments(
    img: &mut impl ImageStore,
    h: &Header,
    mut map: impl FnMut(u64, usize) -> Result<IoRegion>,
) -> Result<()> {
    let mut chunk = [0u8; 256];
    for i in 0..h.phnum {
        let s = segment(img, h, i)?;
        if s.p_type != PT_LOAD || s.memsz == 0 {
            continue;
        }
        if s.filesz > s.memsz {
            return Err(Error::InvalidImage);
        }
        let memsz = usize::try_from(s.memsz).map_err(|_| Error::InvalidImage)?;
        let filesz = s.filesz as usize;
        let io = map(s.paddr, memsz)?;
        io.check(0, memsz)?;
        let mut done = 0;
        while done < filesz {
            let n = chunk.len().min(filesz - done);
            let at = s
                .offset
                .checked_add(done as u64)
                .ok_or(Error::InvalidImage)?;
            img.read_at(at, &mut chunk[..n])?;
            io.write_bytes(done, &chunk[..n])?;
            done += n;
        }
        io.fill(filesz, 0, memsz - filesz)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ELF64 header and one section header, which is also the section
    /// name string table.
    fn elf64_with_strtab(strtab_offset: u64, name_off: u32) -> [u8; 128] {
        let mut f = [0u8; 128];
        f[..7].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1]);
        f[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        f[40..48].copy_from_slice(&64u64.to_le_bytes()); // e_shoff
        f[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
        f[60..62].copy_from_slice(&1u16.to_le_bytes()); // e_shnum
        f[64..68].copy_from_slice(&name_off.to_le_bytes()); // sh_name
        f[88..96].copy_from_slice(&strtab_offset.to_le_bytes()); // sh_offset
        f[96..104].copy_from_slice(&0x100u64.to_le_bytes()); // sh_size
        f
    }

    #[test]
    fn section_name_offset_overflow_is_an_error() {
        // Found by fuzzing: the name's file offset overflowed u64.
        let f = elf64_with_strtab(u64::MAX, 1);
        let mut img = &f[..];
        let h = header(&mut img).unwrap();
        assert_eq!(
            find_section(&mut img, &h, b".resource_table"),
            Err(Error::InvalidImage)
        );
    }
}
