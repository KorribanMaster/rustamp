// SPDX-License-Identifier: BSD-3-Clause
//! Load a synthetic firmware ELF, then run rpmsg between the "host" and the
//! "firmware" (a thread) over the resource table the host prepared.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rustamp_metal::IoRegion;
use rustamp_metal::shmem::ShmemPool;
use rustamp_remoteproc::resource_table;
use rustamp_remoteproc::rsc::{
    FW_RSC_ADDR_ANY, FwRscCarveout, FwRscVdev, FwRscVdevVring, RSC_NOTIFY_ID_ANY, ResourceTable,
};
use rustamp_remoteproc::{Error, Remoteproc, RemoteprocOps, RprocVirtio, State};
use rustamp_rpmsg::{ADDR_ANY, HostConfig, Options, Received, RpmsgVirtio};

use common::{Seg, build_elf32};

const MEM_SIZE: usize = 0x10_0000;
const CODE_PA: u32 = 0x1000;
const RSC_PA: u32 = 0x3000;
const ENTRY: u32 = 0x1041;

resource_table! {
    static FW_TABLE: FwTable = {
        mem: FwRscCarveout = FwRscCarveout::new("fw", CODE_PA, CODE_PA, 0x2000, 0),
        vdev: FwRscVdev<2> = FwRscVdev::new(7, RSC_NOTIFY_ID_ANY, 1, [
            FwRscVdevVring::new(FW_RSC_ADDR_ANY, 4096, 16, RSC_NOTIFY_ID_ANY),
            FwRscVdevVring::new(FW_RSC_ADDR_ANY, 4096, 16, RSC_NOTIFY_ID_ANY),
        ]),
    };
}

fn table_bytes() -> Vec<u8> {
    let io = FW_TABLE.io_region();
    let mut v = vec![0u8; FW_TABLE.size()];
    io.read_bytes(0, &mut v).unwrap();
    v
}

fn firmware_image() -> Vec<u8> {
    build_elf32(
        ENTRY,
        &[
            (
                ".text",
                Seg {
                    paddr: CODE_PA,
                    data: b"\x00\xbf firmware code".to_vec(),
                    memsz: 0x200,
                },
            ),
            (
                ".resource_table",
                Seg {
                    paddr: RSC_PA,
                    data: table_bytes(),
                    memsz: FW_TABLE.size() as u32,
                },
            ),
        ],
    )
}

/// Platform ops over one simulated memory, identity-mapped.
struct SimOps {
    mem: IoRegion,
    started_at: Option<u64>,
    running: bool,
}

impl RemoteprocOps for SimOps {
    fn mmap(&mut self, da: u64, size: usize) -> rustamp_remoteproc::Result<IoRegion> {
        Ok(self.mem.subregion(da as usize, size)?)
    }
    fn start(&mut self, entry: u64) -> rustamp_remoteproc::Result<()> {
        self.started_at = Some(entry);
        self.running = true;
        Ok(())
    }
    fn stop(&mut self) -> rustamp_remoteproc::Result<()> {
        self.running = false;
        Ok(())
    }
}

fn host_rproc() -> Remoteproc<SimOps> {
    let mem = IoRegion::alloc(MEM_SIZE, 0);
    mem.fill(0, 0xaa, MEM_SIZE).unwrap();
    Remoteproc::new(SimOps {
        mem,
        started_at: None,
        running: false,
    })
}

#[test]
fn load_places_segments_and_parses_table() {
    let mut rp = host_rproc();
    rp.config().unwrap();
    assert_eq!(rp.start(), Err(Error::InvalidState));
    let img = firmware_image();
    let info = rp.load(&mut img.as_slice()).unwrap();
    assert_eq!(info.entry, ENTRY as u64);
    assert_eq!(info.resource_table, Some((RSC_PA as u64, FW_TABLE.size())));
    assert_eq!(rp.state(), State::Ready);

    let mem = rp.ops().mem.clone();
    let mut code = [0u8; 16];
    mem.read_bytes(CODE_PA as usize, &mut code).unwrap();
    assert_eq!(&code, b"\x00\xbf firmware code");
    // .bss part of the segment is zeroed; memory after it is untouched.
    assert_eq!(mem.read_u8(CODE_PA as usize + 0x1ff), Ok(0));
    assert_eq!(mem.read_u8(CODE_PA as usize + 0x200), Ok(0xaa));

    // Notify ids were assigned by the host.
    let v = rp.resource_table().unwrap().find_vdev(0).unwrap();
    assert_eq!(v.notifyid(), 0);
    assert_eq!(v.vring(0).unwrap().notifyid, 1);
    assert_eq!(v.vring(1).unwrap().notifyid, 2);

    rp.start().unwrap();
    assert_eq!(rp.ops().started_at, Some(ENTRY as u64));
    assert_eq!(rp.state(), State::Running);
    rp.report_crash();
    assert_eq!(rp.state(), State::Error);
    rp.stop().unwrap();
    assert!(!rp.ops().running);
    rp.shutdown().unwrap();
    assert_eq!(rp.state(), State::Offline);
    assert!(rp.resource_table().is_none());
}

#[test]
fn corrupt_images_are_rejected() {
    let mut rp = host_rproc();
    let mut img = firmware_image();
    assert_eq!(rp.load(&mut &img[..40]).unwrap_err(), Error::InvalidImage);
    img[5] = 2; // big-endian
    assert_eq!(
        rp.load(&mut img.as_slice()).unwrap_err(),
        Error::UnsupportedImage
    );
    img[5] = 1;
    // Program header claims a segment far past the end of the file.
    img[52 + 4..52 + 8].copy_from_slice(&0x0fff_0000u32.to_le_bytes());
    assert_eq!(
        rp.load(&mut img.as_slice()).unwrap_err(),
        Error::InvalidImage
    );
}

#[test]
fn rpmsg_over_loaded_resource_table() {
    let mut rp = host_rproc();
    let img = firmware_image();
    rp.load(&mut img.as_slice()).unwrap();
    let mem = rp.ops().mem.clone();

    let host_kicks = Arc::new(AtomicU32::new(0));
    let hk = host_kicks.clone();
    let mut pool = ShmemPool::new(mem.clone(), 0x2_0000, MEM_SIZE - 0x2_0000).unwrap();
    let vdev = rp
        .create_vdev(0, mem.clone(), Some(&mut pool), move |_id: u32| {
            hk.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
    // Vrings were allocated and written into the firmware's table.
    let table = ResourceTable::new(mem.clone(), RSC_PA as usize, FW_TABLE.size()).unwrap();
    let v = table.find_vdev(0).unwrap();
    assert_eq!(v.vring(0).unwrap().da, 0x2_0000);
    assert_ne!(v.vring(1).unwrap().da, FW_RSC_ADDR_ANY);
    rp.start().unwrap();

    // The firmware: finds its table where it was loaded and runs rpmsg.
    let fw_mem = mem.clone();
    let firmware = std::thread::spawn(move || {
        let table = ResourceTable::new(fw_mem.clone(), RSC_PA as usize, FW_TABLE.size()).unwrap();
        let transport =
            RprocVirtio::new_device(table.find_vdev(0).unwrap(), fw_mem.clone(), |_id: u32| {});
        let opts = Options {
            delay_us: |_| std::thread::yield_now(),
            ..Options::default()
        };
        let rpmsg: RpmsgVirtio<_> = RpmsgVirtio::new_remote(transport, fw_mem, opts).unwrap();
        let ep = rpmsg
            .create_endpoint("rpmsg-client-sample", ADDR_ANY, ADDR_ANY)
            .unwrap();
        let mut n = 0;
        loop {
            match rpmsg.try_recv().unwrap() {
                Some(Received::Message(m)) => {
                    if m.data() == b"bye" {
                        return n;
                    }
                    rpmsg.send_to(ep, m.src(), m.data()).unwrap();
                    n += 1;
                }
                Some(_) => {}
                None => std::thread::yield_now(),
            }
        }
    });

    let opts = Options {
        delay_us: |_| std::thread::yield_now(),
        ..Options::default()
    };
    let host: RpmsgVirtio<_> =
        RpmsgVirtio::new_host(vdev, mem, pool, HostConfig::default(), opts).unwrap();
    let ev = loop {
        if let Some(Received::NameService(ev)) = host.try_recv().unwrap() {
            break ev;
        }
        std::thread::yield_now();
    };
    assert_eq!(&ev.name, "rpmsg-client-sample");
    let ep = host
        .create_endpoint("rpmsg-client-sample", ADDR_ANY, ev.addr)
        .unwrap();
    for i in 0..1000u32 {
        host.send(ep, &i.to_le_bytes()).unwrap();
        let reply = loop {
            if let Some(Received::Message(m)) = host.try_recv().unwrap() {
                break m.data().to_vec();
            }
            std::thread::yield_now();
        };
        assert_eq!(reply, i.to_le_bytes());
    }
    host.send(ep, b"bye").unwrap();
    assert_eq!(firmware.join().unwrap(), 1000);
    assert!(host_kicks.load(Ordering::Relaxed) > 0);
}
