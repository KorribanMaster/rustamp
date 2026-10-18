// SPDX-License-Identifier: BSD-3-Clause
//! Both ends of an rpmsg link while the fuzzer rewrites descriptors and
//! message headers in shared memory, the way a hostile peer could.
#![no_main]

#[path = "../../crates/rpmsg/tests/common/mod.rs"]
mod common;

use arbitrary::Arbitrary;
use common::{SHM_PHYS, SHM_SIZE, Shared, VRING_NUM};
use libfuzzer_sys::fuzz_target;
use rustamp_metal::IoRegion;
use rustamp_rpmsg::wire::HDR_SIZE;
use rustamp_rpmsg::{ADDR_ANY, HostConfig, Options, Received, RpmsgVirtio, VIRTIO_RPMSG_F_NS};

type Dev = RpmsgVirtio<common::Sim>;

/// Offsets of the two vrings in the shared region (see `common`).
const RING_OFF: [usize; 2] = [0x1000, 0x5000];

#[derive(Arbitrary, Debug)]
enum Act {
    Send(bool, Vec<u8>),
    Desc {
        q: bool,
        i: u16,
        addr_off: u32,
        len: u32,
    },
    Hdr {
        q: bool,
        i: u16,
        hdr: [u8; HDR_SIZE],
    },
    Recv(bool),
}

fn recv(dev: &Dev) {
    for _ in 0..4 {
        match dev.try_recv() {
            Ok(Some(Received::Message(m))) => {
                assert_eq!(m.data().len(), m.len());
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
}

thread_local! {
    // `IoRegion::alloc` leaks by design; reuse one region across runs.
    static SHM: IoRegion = IoRegion::alloc(SHM_SIZE, SHM_PHYS);
}

fuzz_target!(|acts: Vec<Act>| {
    let io = SHM.with(|io| io.clone());
    io.fill(0, 0, SHM_SIZE).unwrap();
    let sh = Shared::new_in(io, 1 << VIRTIO_RPMSG_F_NS);
    let host = Dev::new_host(
        sh.host(),
        sh.io.clone(),
        sh.pool(),
        HostConfig::default(),
        Options::default(),
    )
    .unwrap();
    let remote = Dev::try_new_remote(sh.remote(), sh.io.clone(), Options::default()).unwrap();
    let r_ep = remote.create_endpoint("svc", ADDR_ANY, ADDR_ANY).unwrap();
    let Some(Received::NameService(ev)) = host.try_recv().unwrap() else {
        panic!("no announcement")
    };
    let h_ep = host.create_endpoint("svc", ADDR_ANY, ev.addr).unwrap();
    remote.set_endpoint_dest(r_ep, h_ep.addr()).unwrap();

    for a in acts.into_iter().take(128) {
        match a {
            Act::Send(true, d) => {
                let _ = host.try_send(h_ep, &d);
            }
            Act::Send(false, d) => {
                let _ = remote.try_send(r_ep, &d);
            }
            Act::Desc {
                q,
                i,
                addr_off,
                len,
            } => {
                let o = RING_OFF[q as usize] + 16 * (i % VRING_NUM) as usize;
                // Addresses around the shared region, including just outside.
                let addr = SHM_PHYS - 0x100 + (addr_off as u64 % (SHM_SIZE as u64 + 0x200));
                sh.io.write_u64(o, addr).unwrap();
                sh.io.write_u32(o + 8, len).unwrap();
            }
            Act::Hdr { q, i, hdr } => {
                let o = RING_OFF[q as usize] + 16 * (i % VRING_NUM) as usize;
                let off = sh.io.read_u64(o).unwrap().wrapping_sub(SHM_PHYS) as usize;
                if (0x10000..=SHM_SIZE - HDR_SIZE).contains(&off) {
                    sh.io.write_bytes(off, &hdr).unwrap();
                }
            }
            Act::Recv(true) => recv(&host),
            Act::Recv(false) => recv(&remote),
        }
    }
});
