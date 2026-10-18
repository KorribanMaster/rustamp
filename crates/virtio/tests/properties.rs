// SPDX-License-Identifier: BSD-3-Clause
//! Property tests: vring geometry, a model of the driver/device queue pair,
//! and a hostile peer scribbling over the ring.

use std::collections::VecDeque;

use proptest::prelude::*;
use rustamp_metal::IoRegion;
use rustamp_virtio::{
    AvailBuf, Buffer, DeviceQueue, DriverQueue, Error, QueueConfig, Used, ring::VringLayout,
};

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

/// `vring_size()` from the C headers.
fn c_vring_size(num: usize, align: usize) -> usize {
    ((16 * num + 2 * (3 + num) + align - 1) & !(align - 1)) + 2 * 3 + 8 * num
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn layout_matches_c(num in any::<u16>(), align_log in 0u32..16, align_noise in any::<bool>()) {
        let align = (1usize << align_log) + usize::from(align_noise && align_log > 1);
        let valid = num != 0 && num.is_power_of_two() && align >= 4 && align.is_power_of_two();
        match VringLayout::new(num, align) {
            Ok(l) => {
                prop_assert!(valid);
                prop_assert_eq!(l.size(), c_vring_size(num as usize, align));
                prop_assert!(l.avail_event() + 2 <= l.size());
                prop_assert_eq!(l.used() % align, 0);
            }
            Err(e) => {
                prop_assert!(!valid);
                prop_assert_eq!(e, Error::BadLayout);
            }
        }
    }
}

fn pair(num: u16, event_idx: bool) -> (DriverQueue, DeviceQueue, IoRegion, VringLayout) {
    let layout = VringLayout::new(num, 64).unwrap();
    let io = IoRegion::alloc(layout.size(), 0x4000_0000);
    let cfg = QueueConfig {
        io: io.clone(),
        offset: 0,
        layout,
        notify_id: 0,
    };
    let drv = DriverQueue::new(&cfg, event_idx).unwrap();
    let dev = DeviceQueue::new(&cfg, event_idx).unwrap();
    (drv, dev, io, layout)
}

#[derive(Debug, Clone)]
enum Op {
    Add(Vec<(u32, bool)>),
    PopAvail,
    PushUsed(usize, u32),
    PopUsed,
    DrvDisable,
    DrvEnable,
    DrvKick,
    DevDisable,
    DevEnable,
    DevKick,
    /// Hostile peer only: overwrite ring bytes.
    Scribble(usize, Vec<u8>),
}

fn op(hostile: bool) -> BoxedStrategy<Op> {
    let base = prop_oneof![
        4 => prop::collection::vec((any::<u32>(), any::<bool>()), 1..=4).prop_map(Op::Add),
        4 => Just(Op::PopAvail),
        4 => (any::<usize>(), any::<u32>()).prop_map(|(i, l)| Op::PushUsed(i, l)),
        4 => Just(Op::PopUsed),
        1 => Just(Op::DrvDisable),
        1 => Just(Op::DrvEnable),
        2 => Just(Op::DrvKick),
        1 => Just(Op::DevDisable),
        1 => Just(Op::DevEnable),
        2 => Just(Op::DevKick),
    ];
    if hostile {
        prop_oneof![
            6 => base,
            1 => (any::<usize>(), prop::collection::vec(any::<u8>(), 1..8))
                .prop_map(|(o, b)| Op::Scribble(o, b)),
        ]
        .boxed()
    } else {
        base.boxed()
    }
}

fn ring_size() -> impl Strategy<Value = u16> {
    prop_oneof![Just(1u16), Just(2), Just(4), Just(8), Just(16)]
}

/// A chain as the driver posted it.
type Chain = Vec<Buffer>;

#[derive(Default)]
struct Model {
    /// Posted, not yet taken by the device.
    posted: VecDeque<(u16, Chain)>,
    /// Taken by the device.
    held: Vec<(AvailBuf, Chain)>,
    /// Returned by the device, not yet collected.
    used: VecDeque<(u16, u32, Chain)>,
    next_addr: u64,
    /// The device enabled notifications, found nothing to do and sleeps.
    dev_waiting: bool,
    drv_waiting: bool,
    added_since_kick: bool,
    pushed_since_kick: bool,
    /// Buffers were added (pushed) while the peer sleeps, and not yet kicked.
    added_while_waiting: bool,
    pushed_while_waiting: bool,
}

impl Model {
    fn outstanding(&self) -> usize {
        self.posted.iter().map(|c| c.1.len()).sum::<usize>()
            + self.held.iter().map(|c| c.1.len()).sum::<usize>()
            + self.used.iter().map(|c| c.2.len()).sum::<usize>()
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn queue_pair_follows_model(
        num in ring_size(),
        event_idx in any::<bool>(),
        ops in prop::collection::vec(op(false), 1..200),
    ) {
        let (mut drv, mut dev, _io, _) = pair(num, event_idx);
        let mut m = Model::default();
        for op in ops {
            match op {
                Op::Add(spec) => {
                    let chain: Chain = spec
                        .iter()
                        .map(|&(len, w)| {
                            m.next_addr += 1;
                            Buffer { addr: m.next_addr, len, device_writable: w }
                        })
                        .collect();
                    let free = num as usize - m.outstanding();
                    let r = drv.add(&chain);
                    if chain.len() > num as usize {
                        prop_assert_eq!(r, Err(Error::Metal(rustamp_metal::Error::InvalidParam)));
                    } else if chain.len() > free {
                        prop_assert_eq!(r, Err(Error::QueueFull));
                    } else {
                        let head = r.unwrap();
                        prop_assert!(head < num);
                        m.posted.push_back((head, chain));
                        m.added_since_kick = true;
                        m.added_while_waiting |= m.dev_waiting;
                    }
                }
                Op::PopAvail => match dev.pop_avail().unwrap() {
                    Some(b) => {
                        let (head, chain) = m.posted.pop_front().expect("device took a buffer nobody posted");
                        prop_assert_eq!(b.head(), head);
                        let descs: Vec<_> = dev.chain(&b).collect::<Result<_, _>>().unwrap();
                        prop_assert_eq!(descs.len(), chain.len());
                        for (d, c) in descs.iter().zip(&chain) {
                            prop_assert_eq!((d.addr, d.len, d.is_device_writable()), (c.addr, c.len, c.device_writable));
                        }
                        m.held.push((b, chain));
                        m.dev_waiting = false;
                        m.added_while_waiting = false;
                    }
                    None => prop_assert!(m.posted.is_empty()),
                },
                Op::PushUsed(i, len) => {
                    if !m.held.is_empty() {
                        let (b, chain) = m.held.remove(i % m.held.len());
                        let head = b.head();
                        dev.push_used(b, len).unwrap();
                        m.used.push_back((head, len, chain));
                        m.pushed_since_kick = true;
                        m.pushed_while_waiting |= m.drv_waiting;
                    }
                }
                Op::PopUsed => match drv.pop_used().unwrap() {
                    Some(u) => {
                        let (head, len, chain) = m.used.pop_front().expect("driver got a buffer nobody returned");
                        prop_assert_eq!(u, Used { head, len, addr: chain[0].addr, buf_len: chain[0].len });
                        m.drv_waiting = false;
                        m.pushed_while_waiting = false;
                    }
                    None => prop_assert!(m.used.is_empty()),
                },
                Op::DrvDisable => {
                    drv.disable_interrupts().unwrap();
                    m.drv_waiting = false;
                    m.pushed_while_waiting = false;
                }
                Op::DrvEnable => {
                    let pending = drv.enable_interrupts().unwrap();
                    prop_assert_eq!(pending, !m.used.is_empty());
                    m.drv_waiting = !pending;
                    m.pushed_while_waiting = false;
                }
                Op::DrvKick => {
                    let kick = drv.kick_needed().unwrap();
                    if !m.added_since_kick {
                        prop_assert!(!kick);
                    } else if m.added_while_waiting {
                        // A sleeping device must not miss new buffers.
                        prop_assert!(kick, "lost notification to the device");
                    }
                    m.added_since_kick = false;
                    m.added_while_waiting = false;
                    // A kicked device is awake until it enables notifications again.
                    m.dev_waiting &= !kick;
                }
                Op::DevDisable => {
                    dev.disable_notifications().unwrap();
                    m.dev_waiting = false;
                    m.added_while_waiting = false;
                }
                Op::DevEnable => {
                    let pending = dev.enable_notifications().unwrap();
                    prop_assert_eq!(pending, !m.posted.is_empty());
                    m.dev_waiting = !pending;
                    m.added_while_waiting = false;
                }
                Op::DevKick => {
                    let kick = dev.kick_needed().unwrap();
                    if !m.pushed_since_kick {
                        prop_assert!(!kick);
                    } else if m.pushed_while_waiting {
                        prop_assert!(kick, "lost interrupt to the driver");
                    }
                    m.pushed_since_kick = false;
                    m.pushed_while_waiting = false;
                    m.drv_waiting &= !kick;
                }
                Op::Scribble(..) => unreachable!(),
            }
            prop_assert_eq!(drv.num_free() as usize, num as usize - m.outstanding());
        }
    }
}

fn allowed(e: Error) -> bool {
    matches!(e, Error::InvalidDescriptor | Error::Metal(_))
}

proptest! {
    #![proptest_config(config())]

    /// With a peer writing arbitrary bytes over the ring, both sides only
    /// ever return errors or well-formed results, and never panic.
    // Miri's weak-memory model panics (an internal error, not a UB report)
    // on an atomic store after an overlapping non-atomic byte write from the
    // same thread, which is what the scribbling peer does.
    #[cfg_attr(miri, ignore = "Miri internal error on mixed atomic/non-atomic access")]
    #[test]
    fn hostile_peer_cannot_break_either_side(
        num in ring_size(),
        event_idx in any::<bool>(),
        ops in prop::collection::vec(op(true), 1..200),
    ) {
        let (mut drv, mut dev, io, layout) = pair(num, event_idx);
        let mut held: Vec<AvailBuf> = Vec::new();
        let mut addr = 0u64;
        for op in ops {
            match op {
                Op::Scribble(off, bytes) => {
                    let off = off % layout.size();
                    let n = bytes.len().min(layout.size() - off);
                    io.write_bytes(off, &bytes[..n]).unwrap();
                }
                Op::Add(spec) => {
                    let chain: Vec<Buffer> = spec
                        .iter()
                        .map(|&(len, w)| {
                            addr += 1;
                            Buffer { addr, len, device_writable: w }
                        })
                        .collect();
                    match drv.add(&chain) {
                        Ok(head) => prop_assert!(head < num),
                        Err(e) => prop_assert!(allowed(e) || e == Error::QueueFull, "{e:?}"),
                    }
                }
                Op::PopAvail => match dev.pop_avail() {
                    Ok(Some(b)) => {
                        prop_assert!(b.head() < num);
                        for d in dev.chain(&b).take(num as usize + 1) {
                            if let Err(e) = d {
                                prop_assert!(allowed(e), "{e:?}");
                            }
                        }
                        prop_assert!(dev.chain(&b).count() <= num as usize);
                        held.push(b);
                    }
                    Ok(None) => {}
                    Err(e) => prop_assert!(allowed(e), "{e:?}"),
                },
                Op::PushUsed(i, len) => {
                    if !held.is_empty() {
                        let b = held.remove(i % held.len());
                        dev.push_used(b, len).unwrap();
                    }
                }
                Op::PopUsed => match drv.pop_used() {
                    Ok(Some(u)) => prop_assert!(u.head < num),
                    Ok(None) => {}
                    Err(e) => prop_assert!(allowed(e), "{e:?}"),
                },
                Op::DrvDisable => drv.disable_interrupts().unwrap(),
                Op::DrvEnable => { drv.enable_interrupts().unwrap(); }
                Op::DrvKick => { drv.kick_needed().unwrap(); }
                Op::DevDisable => dev.disable_notifications().unwrap(),
                Op::DevEnable => { dev.enable_notifications().unwrap(); }
                Op::DevKick => { dev.kick_needed().unwrap(); }
            }
            prop_assert!(drv.num_free() <= num);
        }
    }
}
