// SPDX-License-Identifier: BSD-3-Clause
//! Load a synthetic firmware ELF, then run rpmsg between the "host" and the
//! "firmware" (a thread) over the resource table the host prepared.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use openamp_metal::IoRegion;
use openamp_metal::shmem::ShmemPool;
use openamp_remoteproc::resource_table;
use openamp_remoteproc::rsc::{
    FW_RSC_ADDR_ANY, FwRscCarveout, FwRscVdev, FwRscVdevVring, RSC_NOTIFY_ID_ANY, ResourceTable,
};
use openamp_remoteproc::{Error, Remoteproc, RemoteprocOps, RprocVirtio, State};
use openamp_rpmsg::{ADDR_ANY, HostConfig, Options, Received, RpmsgVirtio};

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

struct Seg {
    paddr: u32,
    data: Vec<u8>,
    memsz: u32,
}

/// A minimal ELF32 little-endian ARM executable with one section per
/// segment plus `.shstrtab`.
fn build_elf32(entry: u32, segs: &[(&str, Seg)]) -> Vec<u8> {
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
    fn mmap(&mut self, da: u64, size: usize) -> openamp_remoteproc::Result<IoRegion> {
        Ok(self.mem.subregion(da as usize, size)?)
    }
    fn start(&mut self, entry: u64) -> openamp_remoteproc::Result<()> {
        self.started_at = Some(entry);
        self.running = true;
        Ok(())
    }
    fn stop(&mut self) -> openamp_remoteproc::Result<()> {
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
