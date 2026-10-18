// SPDX-License-Identifier: BSD-3-Clause
//! Property tests: wire encodings, and both sides of a link with a hostile
//! peer rewriting descriptors and message headers.

mod common;

use common::{SHM_PHYS, SHM_SIZE, Shared, VRING_NUM};
use openamp_rpmsg::wire::{HDR_SIZE, Header, NS_MSG_SIZE, NsMsg};
use openamp_rpmsg::{
    ADDR_ANY, Error, HostConfig, Options, Received, RpmsgVirtio, VIRTIO_RPMSG_F_NS,
};
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

type Dev = RpmsgVirtio<common::Sim>;

proptest! {
    #![proptest_config(config())]

    #[test]
    fn header_round_trips(src: u32, dst: u32, reserved: u32, len: u16, flags: u16) {
        let h = Header { src, dst, reserved, len, flags };
        prop_assert_eq!(Header::from_bytes(&h.to_bytes()), h);
    }

    #[test]
    fn header_bytes_round_trip(b: [u8; HDR_SIZE]) {
        prop_assert_eq!(Header::from_bytes(&b).to_bytes(), b);
    }

    #[test]
    fn ns_msg_round_trips(b in prop::collection::vec(any::<u8>(), 0..64)) {
        match NsMsg::from_bytes(&b) {
            Some(m) => {
                prop_assert_eq!(b.len(), NS_MSG_SIZE);
                prop_assert_eq!(NsMsg::from_bytes(&m.to_bytes()), Some(m));
            }
            None => prop_assert_ne!(b.len(), NS_MSG_SIZE),
        }
    }
}

/// Offsets of the two vrings in the shared region (see `common`).
const RING_OFF: [usize; 2] = [0x1000, 0x5000];

#[derive(Debug, Clone)]
enum Act {
    /// The host (true) or the remote (false) sends.
    Send(bool, Vec<u8>),
    /// Overwrite descriptor `i` of queue `q`.
    Desc {
        q: usize,
        i: u16,
        addr_off: i64,
        len: u32,
    },
    /// Overwrite the message header in the buffer descriptor `i` points to.
    Hdr {
        q: usize,
        i: u16,
        hdr: [u8; HDR_SIZE],
    },
    /// The host (true) or the remote (false) receives.
    Recv(bool),
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        3 => (any::<bool>(), prop::collection::vec(any::<u8>(), 0..64)).prop_map(|(h, d)| Act::Send(h, d)),
        1 => (0..2usize, 0..VRING_NUM, -0x200i64..SHM_SIZE as i64 + 0x200, prop_oneof![0..64u32, any::<u32>(), 500..600u32])
            .prop_map(|(q, i, addr_off, len)| Act::Desc { q, i, addr_off, len }),
        2 => (0..2usize, 0..VRING_NUM, any::<[u8; HDR_SIZE]>()).prop_map(|(q, i, hdr)| Act::Hdr { q, i, hdr }),
        4 => any::<bool>().prop_map(Act::Recv),
    ]
}

fn recv(dev: &Dev) -> Result<(), TestCaseError> {
    for _ in 0..4 {
        match dev.try_recv() {
            Ok(Some(Received::Message(m))) => {
                prop_assert_eq!(m.data().len(), m.len());
                prop_assert!(m.len() <= 512 - HDR_SIZE);
                let mut out = [0u8; 600];
                prop_assert_eq!(m.copy_to(&mut out), m.len());
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => prop_assert!(
                matches!(e, Error::InvalidBuffer | Error::Virtio(_) | Error::Metal(_)),
                "{e:?}"
            ),
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn hostile_peer_cannot_break_try_recv(acts in prop::collection::vec(act(), 1..60)) {
        let sh = Shared::new(1 << VIRTIO_RPMSG_F_NS);
        let host = Dev::new_host(sh.host(), sh.io.clone(), sh.pool(), HostConfig::default(), Options::default()).unwrap();
        let remote = Dev::try_new_remote(sh.remote(), sh.io.clone(), Options::default()).unwrap();
        let r_ep = remote.create_endpoint("svc", ADDR_ANY, ADDR_ANY).unwrap();
        let Some(Received::NameService(ev)) = host.try_recv().unwrap() else { panic!() };
        let h_ep = host.create_endpoint("svc", ADDR_ANY, ev.addr).unwrap();
        remote.set_endpoint_dest(r_ep, h_ep.addr()).unwrap();

        for a in acts {
            match a {
                Act::Send(true, d) => { let _ = host.try_send(h_ep, &d); }
                Act::Send(false, d) => { let _ = remote.try_send(r_ep, &d); }
                Act::Desc { q, i, addr_off, len } => {
                    let o = RING_OFF[q] + 16 * i as usize;
                    sh.io.write_u64(o, SHM_PHYS.wrapping_add_signed(addr_off)).unwrap();
                    sh.io.write_u32(o + 8, len).unwrap();
                }
                Act::Hdr { q, i, hdr } => {
                    let o = RING_OFF[q] + 16 * i as usize;
                    let addr = sh.io.read_u64(o).unwrap();
                    let off = addr.wrapping_sub(SHM_PHYS) as usize;
                    // Only into the buffer pool, not over the rings.
                    if (0x10000..=SHM_SIZE - HDR_SIZE).contains(&off) {
                        sh.io.write_bytes(off, &hdr).unwrap();
                    }
                }
                Act::Recv(true) => recv(&host)?,
                Act::Recv(false) => recv(&remote)?,
            }
        }
    }
}
