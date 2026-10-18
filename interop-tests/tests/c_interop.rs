// SPDX-License-Identifier: BSD-3-Clause
//! Rust ↔ C OpenAMP over the same shared memory, in both role pairings.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rustamp_interop_tests::*;
use rustamp_metal::IoRegion;
use rustamp_metal::shmem::ShmemPool;
use rustamp_remoteproc::RprocVirtio;
use rustamp_remoteproc::rsc::ResourceTable;
use rustamp_rpmsg::{ADDR_ANY, HostConfig, Options, Received, RpmsgVirtio};

const VRING_NUM: u32 = 16;
const N: u32 = 20_000;

fn yield_opts() -> Options {
    Options {
        delay_us: |_| std::thread::yield_now(),
        ..Options::default()
    }
}

/// Shared memory carrying the resource table written by the C code.
fn shared_memory() -> (IoRegion, ResourceTable) {
    let io = IoRegion::alloc(SHM_SIZE, SHM_PHYS);
    // SAFETY: the buffer is SHM_SIZE bytes and nothing else uses it yet.
    let len = unsafe {
        c_remote_write_table(io.as_ptr().cast(), SHM_PHYS, VRING_NUM);
        c_rsc_table_size()
    };
    let table = ResourceTable::new(io.clone(), 0, len).expect("C table parses");
    (io, table)
}

#[test]
fn rust_host_c_remote() {
    let (io, table) = shared_memory();
    let vdev = table.find_vdev(0).unwrap();
    assert_eq!((vdev.id(), vdev.dfeatures(), vdev.num_vrings()), (7, 1, 2));

    let va = io.as_ptr() as usize;
    // SAFETY: the C remote uses the buffer only through libmetal for the
    // duration of the call; the region is leaked, so it outlives the thread.
    let remote =
        std::thread::spawn(move || unsafe { c_remote_run(va as *mut _, SHM_PHYS, SHM_SIZE) });

    let kicks = Arc::new(AtomicU32::new(0));
    let k = kicks.clone();
    let transport = RprocVirtio::new_driver(vdev, io.clone(), move |_id: u32| {
        k.fetch_add(1, Ordering::Relaxed);
    });
    let pool = ShmemPool::new(io.clone(), POOL_OFF, SHM_SIZE - POOL_OFF).unwrap();
    let host: RpmsgVirtio<_> =
        RpmsgVirtio::new_host(transport, io, pool, HostConfig::default(), yield_opts()).unwrap();
    assert!(host.support_ns());

    let ev = loop {
        match host.try_recv().unwrap() {
            Some(Received::NameService(ev)) => break ev,
            _ => std::thread::yield_now(),
        }
    };
    assert_eq!(&ev.name, SERVICE);
    let ep = host.create_endpoint(SERVICE, ADDR_ANY, ev.addr).unwrap();

    // Keep up to 8 messages in flight to exercise buffer recycling.
    let (mut sent, mut got) = (0u32, 0u32);
    while got < N {
        while sent < N && sent - got < 8 {
            host.send(ep, &make_msg(sent, 496)).unwrap();
            sent += 1;
        }
        match host.try_recv().unwrap() {
            Some(Received::Message(m)) => {
                assert_eq!(m.data(), &make_msg(got, 496)[..], "echo {got}");
                assert_eq!(m.src(), ev.addr);
                got += 1;
            }
            _ => std::thread::yield_now(),
        }
    }
    host.send(ep, b"quit").unwrap();
    assert_eq!(remote.join().unwrap(), N as i32);
    assert!(kicks.load(Ordering::Relaxed) > 0);
}

#[test]
fn c_host_rust_remote() {
    let (io, table) = shared_memory();
    let va = io.as_ptr() as usize;
    // SAFETY: as above.
    let host =
        std::thread::spawn(move || unsafe { c_host_run(va as *mut _, SHM_PHYS, SHM_SIZE, N) });

    let transport = RprocVirtio::new_device(table.find_vdev(0).unwrap(), io.clone(), |_id: u32| {});
    let remote: RpmsgVirtio<_> = RpmsgVirtio::new_remote(transport, io, yield_opts()).unwrap();
    assert!(remote.support_ns());
    let ep = remote.create_endpoint(SERVICE, ADDR_ANY, ADDR_ANY).unwrap();
    let mut echoed = 0;
    loop {
        match remote.try_recv().unwrap() {
            Some(Received::Message(m)) => {
                if m.data() == b"quit" {
                    break;
                }
                remote.send_to(ep, m.src(), m.data()).unwrap();
                echoed += 1;
            }
            Some(other) => panic!("unexpected {other:?}"),
            None => std::thread::yield_now(),
        }
    }
    assert_eq!(echoed, N);
    assert_eq!(host.join().unwrap(), N as i32, "C host verified every echo");
}
