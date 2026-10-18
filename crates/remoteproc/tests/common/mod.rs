// SPDX-License-Identifier: BSD-3-Clause
//! Test fixtures: a minimal ELF builder.
#![allow(dead_code, unreachable_pub)]

pub struct Seg {
    pub paddr: u32,
    pub data: Vec<u8>,
    pub memsz: u32,
}

/// A minimal ELF32 little-endian ARM executable with one section per
/// segment plus `.shstrtab`.
pub fn build_elf32(entry: u32, segs: &[(&str, Seg)]) -> Vec<u8> {
    let phoff = 52usize;
    let mut data_off = phoff + 32 * segs.len();
    let mut offs = Vec::new();
    for (_, s) in segs {
        data_off = (data_off + 3) & !3;
        offs.push(data_off);
        data_off += s.data.len();
    }
    let mut shstr = vec![0u8];
    let mut name_offs = Vec::new();
    for (n, _) in segs {
        name_offs.push(shstr.len() as u32);
        shstr.extend_from_slice(n.as_bytes());
        shstr.push(0);
    }
    let shstr_name = shstr.len() as u32;
    shstr.extend_from_slice(b".shstrtab\0");
    let shstr_off = data_off;
    let shoff = (shstr_off + shstr.len() + 3) & !3;
    let shnum = segs.len() + 2;

    let mut f = vec![0u8; shoff + 40 * shnum];
    let w16 = |f: &mut Vec<u8>, o: usize, v: u16| f[o..o + 2].copy_from_slice(&v.to_le_bytes());
    let w32 = |f: &mut Vec<u8>, o: usize, v: u32| f[o..o + 4].copy_from_slice(&v.to_le_bytes());
    f[..7].copy_from_slice(&[0x7f, b'E', b'L', b'F', 1, 1, 1]);
    w16(&mut f, 16, 2); // ET_EXEC
    w16(&mut f, 18, 40); // EM_ARM
    w32(&mut f, 20, 1);
    w32(&mut f, 24, entry);
    w32(&mut f, 28, phoff as u32);
    w32(&mut f, 32, shoff as u32);
    w16(&mut f, 40, 52);
    w16(&mut f, 42, 32);
    w16(&mut f, 44, segs.len() as u16);
    w16(&mut f, 46, 40);
    w16(&mut f, 48, shnum as u16);
    w16(&mut f, 50, (shnum - 1) as u16);
    for (i, (_, s)) in segs.iter().enumerate() {
        let p = phoff + 32 * i;
        w32(&mut f, p, 1); // PT_LOAD
        w32(&mut f, p + 4, offs[i] as u32);
        w32(&mut f, p + 8, s.paddr);
        w32(&mut f, p + 12, s.paddr);
        w32(&mut f, p + 16, s.data.len() as u32);
        w32(&mut f, p + 20, s.memsz);
        f[offs[i]..offs[i] + s.data.len()].copy_from_slice(&s.data);
        let sh = shoff + 40 * (i + 1);
        w32(&mut f, sh, name_offs[i]);
        w32(&mut f, sh + 4, 1); // PROGBITS
        w32(&mut f, sh + 12, s.paddr);
        w32(&mut f, sh + 16, offs[i] as u32);
        w32(&mut f, sh + 20, s.data.len() as u32);
    }
    f[shstr_off..shstr_off + shstr.len()].copy_from_slice(&shstr);
    let sh = shoff + 40 * (shnum - 1);
    w32(&mut f, sh, shstr_name);
    w32(&mut f, sh + 4, 3); // STRTAB
    w32(&mut f, sh + 16, shstr_off as u32);
    w32(&mut f, sh + 20, shstr.len() as u32);
    f
}
