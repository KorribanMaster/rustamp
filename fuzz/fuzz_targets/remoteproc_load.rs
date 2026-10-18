// SPDX-License-Identifier: BSD-3-Clause
//! An arbitrary image through `Remoteproc::load` (ELF parsing, segment
//! loading, resource-table parsing and handling), and the same bytes as a
//! resource table.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rustamp_metal::IoRegion;
use rustamp_remoteproc::rsc::ResourceTable;
use rustamp_remoteproc::{Remoteproc, RemoteprocOps};

const MEM_SIZE: usize = 0x2_0000;
/// Inputs longer than this skip the resource-table-only pass.
const TABLE_MAX: usize = 0x1_0000;

thread_local! {
    // `IoRegion::alloc` leaks by design; reuse regions across runs.
    static MEM: IoRegion = IoRegion::alloc(MEM_SIZE, 0);
    static TABLE: IoRegion = IoRegion::alloc(TABLE_MAX, 0);
}

struct Sim {
    mem: IoRegion,
}

impl RemoteprocOps for Sim {
    fn mmap(&mut self, da: u64, size: usize) -> rustamp_remoteproc::Result<IoRegion> {
        let da = usize::try_from(da).map_err(|_| rustamp_remoteproc::Error::NoMemory)?;
        Ok(self.mem.subregion(da, size)?)
    }
    fn start(&mut self, _: u64) -> rustamp_remoteproc::Result<()> {
        Ok(())
    }
    fn stop(&mut self) -> rustamp_remoteproc::Result<()> {
        Ok(())
    }
}

fn walk(t: &ResourceTable) {
    for e in t.entries().take(64).flatten() {
        let _ = t.carveout(&e);
        let _ = t.trace(&e);
        if let Ok(v) = t.vdev(&e) {
            let _ = (v.id(), v.notifyid(), v.dfeatures(), v.status());
            for i in 0..=v.num_vrings() {
                let _ = v.vring(i);
            }
        }
    }
    let _ = t.find_vdev(0);
}

fuzz_target!(|img: &[u8]| {
    let mem = MEM.with(|m| m.clone());
    mem.fill(0, 0, MEM_SIZE).unwrap();
    let mut rp = Remoteproc::new(Sim { mem });
    if rp.load(&mut &img[..]).is_ok() {
        if let Some(t) = rp.resource_table() {
            walk(t);
        }
    }

    if img.len() <= TABLE_MAX {
        let io = TABLE.with(|t| t.clone());
        io.write_bytes(0, img).unwrap();
        if let Ok(t) = ResourceTable::new(io, 0, img.len()) {
            walk(&t);
        }
    }
});
