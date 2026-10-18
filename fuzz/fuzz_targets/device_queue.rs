// SPDX-License-Identifier: BSD-3-Clause
//! The device side of a virtqueue over a ring the fuzzer controls: initial
//! contents, and rewrites between operations.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use rustamp_metal::IoRegion;
use rustamp_virtio::ring::VringLayout;
use rustamp_virtio::{AvailBuf, DeviceQueue, QueueConfig};

#[derive(Arbitrary, Debug)]
enum Op {
    Pop,
    Push(u8, u32),
    Kick,
    Enable,
    Disable,
    Write(u16, Vec<u8>),
}

#[derive(Arbitrary, Debug)]
struct Input {
    num_log2: u8,
    event_idx: bool,
    ring: Vec<u8>,
    ops: Vec<Op>,
}

thread_local! {
    // `IoRegion::alloc` leaks by design; reuse one region across runs.
    static RING: IoRegion = IoRegion::alloc(0x1000, 0x4000_0000);
}

fuzz_target!(|inp: Input| {
    let num = 1u16 << (inp.num_log2 % 6);
    let layout = VringLayout::new(num, 64).unwrap();
    let io = RING.with(|r| r.subregion(0, layout.size()).unwrap());
    io.fill(0, 0, layout.size()).unwrap();
    let n = inp.ring.len().min(layout.size());
    io.write_bytes(0, &inp.ring[..n]).unwrap();
    let cfg = QueueConfig {
        io: io.clone(),
        offset: 0,
        layout,
        notify_id: 0,
    };
    let Ok(mut dev) = DeviceQueue::new(&cfg, inp.event_idx) else {
        return;
    };
    let mut held: Vec<AvailBuf> = Vec::new();
    for op in inp.ops.into_iter().take(256) {
        match op {
            Op::Pop => {
                if let Ok(Some(b)) = dev.pop_avail() {
                    assert!(b.head() < num);
                    assert!(dev.chain(&b).take(num as usize + 1).count() <= num as usize);
                    held.push(b);
                }
            }
            Op::Push(i, len) => {
                if !held.is_empty() {
                    let b = held.remove(i as usize % held.len());
                    let _ = dev.push_used(b, len);
                }
            }
            Op::Kick => {
                let _ = dev.kick_needed();
            }
            Op::Enable => {
                let _ = dev.enable_notifications();
            }
            Op::Disable => {
                let _ = dev.disable_notifications();
            }
            Op::Write(off, bytes) => {
                let off = off as usize % layout.size();
                let n = bytes.len().min(layout.size() - off);
                io.write_bytes(off, &bytes[..n]).unwrap();
            }
        }
    }
});
