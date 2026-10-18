// SPDX-License-Identifier: BSD-3-Clause
//! Property tests: resource tables and ELF images from a hostile source.

mod common;

use common::{Seg, build_elf32};
use openamp_metal::IoRegion;
use openamp_remoteproc::elf;
use openamp_remoteproc::resource_table;
use openamp_remoteproc::rsc::{
    FW_RSC_ADDR_ANY, FwRscCarveout, FwRscVdev, FwRscVdevVring, RSC_NOTIFY_ID_ANY, ResourceTable,
};
use openamp_remoteproc::{Remoteproc, RemoteprocOps};
use proptest::prelude::*;

/// The default configuration (honouring `PROPTEST_CASES`), cut down to a
/// few cases under Miri.
fn config() -> ProptestConfig {
    let mut c = ProptestConfig::default();
    if cfg!(miri) {
        c.cases = 2;
        c.failure_persistence = None;
    }
    c
}

const MEM_SIZE: usize = 0x2_0000;

resource_table! {
    static FW_TABLE: FwTable = {
        mem: FwRscCarveout = FwRscCarveout::new("fw", 0x1000, 0x1000, 0x2000, 0),
        vdev: FwRscVdev<2> = FwRscVdev::new(7, RSC_NOTIFY_ID_ANY, 1, [
            FwRscVdevVring::new(FW_RSC_ADDR_ANY, 4096, 16, RSC_NOTIFY_ID_ANY),
            FwRscVdevVring::new(FW_RSC_ADDR_ANY, 4096, 16, RSC_NOTIFY_ID_ANY),
        ]),
    };
}

fn table_bytes() -> Vec<u8> {
    let mut v = vec![0u8; FW_TABLE.size()];
    FW_TABLE.io_region().read_bytes(0, &mut v).unwrap();
    v
}

fn firmware_image() -> Vec<u8> {
    build_elf32(
        0x1041,
        &[
            (
                ".text",
                Seg {
                    paddr: 0x1000,
                    data: b"\x00\xbf firmware code".to_vec(),
                    memsz: 0x200,
                },
            ),
            (
                ".resource_table",
                Seg {
                    paddr: 0x3000,
                    data: table_bytes(),
                    memsz: FW_TABLE.size() as u32,
                },
            ),
        ],
    )
}

/// Walks everything a host would read from a table.
fn walk(t: &ResourceTable) {
    for e in t.entries().take(64) {
        let Ok(e) = e else { continue };
        let _ = t.carveout(&e);
        let _ = t.trace(&e);
        if let Ok(v) = t.vdev(&e) {
            let _ = (
                v.id(),
                v.notifyid(),
                v.dfeatures(),
                v.gfeatures(),
                v.status(),
            );
            for i in 0..v.num_vrings() + 1 {
                let _ = v.vring(i);
            }
        }
    }
    for n in 0..3 {
        let _ = t.find_vdev(n);
    }
}

/// Mutations applied to a valid byte image.
fn mutate(mut img: Vec<u8>, edits: &[(usize, u8)], cut: Option<usize>) -> Vec<u8> {
    for &(at, v) in edits {
        let n = img.len();
        img[at % n] = v;
    }
    if let Some(c) = cut {
        img.truncate(c % (img.len() + 1));
    }
    img
}

fn edits() -> impl Strategy<Value = Vec<(usize, u8)>> {
    prop::collection::vec((any::<usize>(), any::<u8>()), 0..8)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn resource_table_from_random_bytes(
        mut b in prop::collection::vec(any::<u8>(), 0..512),
        plausible in any::<bool>(),
        num in 0u32..8,
    ) {
        if plausible && b.len() >= 16 {
            // Version 1 and a small count, so parsing gets past the header.
            b[..4].copy_from_slice(&1u32.to_le_bytes());
            b[4..8].copy_from_slice(&num.to_le_bytes());
        }
        let io = IoRegion::alloc(b.len().max(1), 0);
        io.write_bytes(0, &b).unwrap();
        if let Ok(t) = ResourceTable::new(io, 0, b.len()) {
            walk(&t);
        }
    }

    #[test]
    fn resource_table_from_mutated_table(edits in edits(), cut: Option<usize>) {
        let b = mutate(table_bytes(), &edits, cut);
        let io = IoRegion::alloc(b.len().max(1), 0);
        io.write_bytes(0, &b).unwrap();
        if let Ok(t) = ResourceTable::new(io, 0, b.len()) {
            walk(&t);
        }
    }

    #[test]
    fn elf_from_random_bytes(b in prop::collection::vec(any::<u8>(), 0..256), magic in any::<bool>()) {
        let mut b = b;
        if magic && b.len() >= 7 {
            let class = 1 + (b[4] & 1);
            b[..7].copy_from_slice(&[0x7f, b'E', b'L', b'F', class, 1, 1]);
        }
        load_elf(&b);
    }

    #[test]
    fn elf_from_mutated_image(edits in edits(), cut: Option<usize>) {
        load_elf(&mutate(firmware_image(), &edits, cut));
    }

    #[test]
    fn remoteproc_load_of_mutated_image(edits in edits(), cut: Option<usize>) {
        let img = mutate(firmware_image(), &edits, cut);
        let mem = IoRegion::alloc(MEM_SIZE, 0);
        let mut rp = Remoteproc::new(Sim { mem });
        if let Ok(info) = rp.load(&mut img.as_slice()) {
            prop_assert_eq!(info.resource_table.is_some(), rp.resource_table().is_some());
            if let Some(t) = rp.resource_table() {
                walk(t);
            }
        }
    }
}

/// Parses and loads an image into a small memory; must never panic.
fn load_elf(b: &[u8]) {
    let mem = IoRegion::alloc(MEM_SIZE, 0);
    let mut img = b;
    let Ok(h) = elf::header(&mut img) else {
        return;
    };
    for i in 0..h.phnum.min(64) {
        let _ = elf::segment(&mut img, &h, i);
    }
    let _ = elf::find_section(&mut img, &h, b".resource_table");
    let _ = elf::load_segments(&mut img, &h, |da, size| {
        let da = usize::try_from(da).map_err(|_| openamp_metal::Error::OutOfBounds)?;
        Ok(mem.subregion(da, size)?)
    });
}

struct Sim {
    mem: IoRegion,
}

impl RemoteprocOps for Sim {
    fn mmap(&mut self, da: u64, size: usize) -> openamp_remoteproc::Result<IoRegion> {
        let da = usize::try_from(da).map_err(|_| openamp_remoteproc::Error::NoMemory)?;
        Ok(self.mem.subregion(da, size)?)
    }
    fn start(&mut self, _: u64) -> openamp_remoteproc::Result<()> {
        Ok(())
    }
    fn stop(&mut self) -> openamp_remoteproc::Result<()> {
        Ok(())
    }
}
