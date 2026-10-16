// SPDX-License-Identifier: BSD-3-Clause
//! Host and remote rpmsg devices talking over shared memory.

mod common;

use common::Shared;
use openamp_rpmsg::{
    ADDR_ANY, Error, HostConfig, NsKind, Options, Received, RpmsgVirtio, VIRTIO_RPMSG_F_NS,
};

type Dev = RpmsgVirtio<common::Sim>;

fn pair(dfeatures: u32) -> (std::sync::Arc<Shared>, Dev, Dev) {
    let sh = Shared::new(dfeatures);
    let host = Dev::new_host(
        sh.host(),
        sh.io.clone(),
        sh.pool(),
        HostConfig::default(),
        Options::default(),
    )
    .unwrap();
    let remote = Dev::try_new_remote(sh.remote(), sh.io.clone(), Options::default()).unwrap();
    (sh, host, remote)
}

fn expect_ns(dev: &Dev) -> openamp_rpmsg::NsEvent {
    match dev.try_recv().unwrap() {
        Some(Received::NameService(e)) => e,
        other => panic!("expected NS event, got {other:?}"),
    }
}

#[test]
fn remote_before_host_is_not_ready() {
    let sh = Shared::new(1);
    assert_eq!(
        Dev::try_new_remote(sh.remote(), sh.io.clone(), Options::default()).unwrap_err(),
        Error::NotReady
    );
    assert_eq!(
        Dev::try_new_remote(sh.host(), sh.io.clone(), Options::default()).unwrap_err(),
        Error::InvalidParam
    );
}

#[test]
fn name_service_binding_and_messaging() {
    let (sh, host, remote) = pair(1 << VIRTIO_RPMSG_F_NS);
    assert!(host.support_ns() && remote.support_ns());
    // Host posted all receive buffers and kicked queue 0 once.
    assert_eq!(sh.kicks[0].load(std::sync::atomic::Ordering::SeqCst), 1);

    let r_ep = remote.create_endpoint("rpmsg-svc", ADDR_ANY, ADDR_ANY).unwrap();
    assert_eq!(r_ep.addr(), 1024);
    let ev = expect_ns(&host);
    assert_eq!((ev.name, ev.addr, ev.kind).1, 1024);
    assert_eq!(&ev.name, "rpmsg-svc");
    assert_eq!(ev.kind, NsKind::Create);

    // Bind a host endpoint to the announced service: no announcement back.
    let h_ep = host.create_endpoint("rpmsg-svc", ADDR_ANY, ev.addr).unwrap();
    assert!(host.is_ready(h_ep));
    assert!(!remote.is_ready(r_ep));

    host.send(h_ep, b"ping").unwrap();
    {
        let Some(Received::Message(m)) = remote.try_recv().unwrap() else {
            panic!("no message")
        };
        assert_eq!(m.data(), b"ping");
        assert_eq!(m.src(), h_ep.addr());
        assert_eq!(m.endpoint(), r_ep);
    }
    // First message bound the remote endpoint to the sender.
    assert_eq!(remote.endpoint_dest(r_ep).unwrap(), Some(h_ep.addr()));
    remote.send(r_ep, b"pong").unwrap();
    let Some(Received::Message(m)) = host.try_recv().unwrap() else {
        panic!("no reply")
    };
    assert_eq!(m.data(), b"pong");
    drop(m);
    assert!(host.try_recv().unwrap().is_none());

    // Destruction is announced and unbinds the host endpoint.
    remote.destroy_endpoint(r_ep).unwrap();
    let ev = expect_ns(&host);
    assert_eq!(ev.kind, NsKind::Destroy);
    assert_eq!(host.send(h_ep, b"x"), Err(Error::NotReady));
    assert_eq!(remote.send(r_ep, b"x"), Err(Error::InvalidEndpoint));
}

#[test]
fn without_ns_feature_nothing_is_announced() {
    let (_sh, host, remote) = pair(0);
    assert!(!host.support_ns());
    let r_ep = remote.create_endpoint("quiet", 0x40, ADDR_ANY).unwrap();
    assert!(host.try_recv().unwrap().is_none());
    let h_ep = host.create_endpoint("", ADDR_ANY, 0x40).unwrap();
    host.send(h_ep, b"hi").unwrap();
    let Some(Received::Message(m)) = remote.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!((m.data(), m.endpoint()), (&b"hi"[..], r_ep));
}

#[test]
fn held_buffers_exhaust_and_recover() {
    let (_sh, host, remote) = pair(0);
    let r_ep = remote.create_endpoint("", 0x40, ADDR_ANY).unwrap();
    let h_ep = host.create_endpoint("", ADDR_ANY, 0x40).unwrap();
    let n = common::VRING_NUM as usize;
    for i in 0..n {
        host.try_send(h_ep, &[i as u8; 8]).unwrap();
    }
    // The host's transmit ring is full until the remote returns buffers.
    assert_eq!(host.try_send(h_ep, b"x"), Err(Error::NoBuffer));
    let mut held = Vec::new();
    while let Some(Received::Message(m)) = remote.try_recv().unwrap() {
        held.push(m);
    }
    assert_eq!(held.len(), n);
    assert_eq!(held[3].data(), &[3u8; 8]);
    assert_eq!(host.try_send(h_ep, b"x"), Err(Error::NoBuffer));
    held.truncate(n - 2);
    host.try_send(h_ep, b"a").unwrap();
    host.try_send(h_ep, b"b").unwrap();
    assert_eq!(host.try_send(h_ep, b"c"), Err(Error::NoBuffer));
    drop(held);
    let _ = r_ep;
}

#[test]
fn size_limits_and_zero_copy() {
    let (_sh, host, remote) = pair(0);
    let _r = remote.create_endpoint("", 0x40, ADDR_ANY).unwrap();
    let h_ep = host.create_endpoint("", ADDR_ANY, 0x40).unwrap();
    assert_eq!(host.try_send(h_ep, &[0; 497]), Err(Error::MessageTooLarge));
    host.try_send(h_ep, &[7; 496]).unwrap();

    let mut tx = host.get_tx_buffer(false).unwrap();
    assert_eq!(tx.capacity(), 496);
    tx.as_mut_slice()[..5].copy_from_slice(b"inpla");
    tx.write(5, b"ce").unwrap();
    assert_eq!(tx.write(495, b"xx"), Err(Error::MessageTooLarge));
    tx.send(h_ep, 7).unwrap();

    // An unsent buffer goes back to the pool when dropped.
    drop(host.get_tx_buffer(false).unwrap());

    let Some(Received::Message(m)) = remote.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!(m.data(), &[7u8; 496][..]);
    drop(m);
    let Some(Received::Message(m)) = remote.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!(m.data(), b"inplace");
    let mut out = [0u8; 4];
    assert_eq!(m.copy_to(&mut out), 4);
    assert_eq!(&out, b"inpl");
}

#[test]
fn messages_for_unknown_endpoints_are_dropped() {
    let (_sh, host, remote) = pair(0);
    let r_ep = remote.create_endpoint("", 0x40, ADDR_ANY).unwrap();
    host.send_offchannel(0x400, 0x999, b"lost", false).unwrap();
    host.send_offchannel(0x400, 0x40, b"found", false).unwrap();
    let Some(Received::Message(m)) = remote.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!((m.data(), m.endpoint()), (&b"found"[..], r_ep));
}

#[test]
fn endpoint_addresses() {
    let (_sh, host, _remote) = pair(1 << VIRTIO_RPMSG_F_NS);
    let a = host.create_endpoint("", ADDR_ANY, 1).unwrap();
    let b = host.create_endpoint("", ADDR_ANY, 1).unwrap();
    assert_eq!((a.addr(), b.addr()), (1024, 1025));
    assert_eq!(
        host.create_endpoint("", 1025, 1).unwrap_err(),
        Error::InvalidAddress
    );
    assert_eq!(
        host.create_endpoint("", 0x35, 1).unwrap_err(),
        Error::InvalidAddress
    );
    assert_eq!(
        host.create_endpoint("", 1024 + 128, 1).unwrap_err(),
        Error::InvalidAddress
    );
    host.destroy_endpoint(a).unwrap();
    // Allocation continues after the last one handed out, like C.
    assert_eq!(host.create_endpoint("", ADDR_ANY, 1).unwrap().addr(), 1026);
    for _ in 0..6 {
        host.create_endpoint("", ADDR_ANY, 1).unwrap();
    }
    assert_eq!(
        host.create_endpoint("", ADDR_ANY, 1).unwrap_err(),
        Error::NoEndpointSlot
    );
}

#[test]
fn threaded_echo_100k() {
    threaded_echo(1 << VIRTIO_RPMSG_F_NS);
}

#[test]
fn threaded_echo_100k_event_idx() {
    threaded_echo(1 << VIRTIO_RPMSG_F_NS | 1 << 29);
}

fn threaded_echo(dfeatures: u32) {
    let sh = Shared::new(dfeatures);
    let sh2 = sh.clone();
    let remote = std::thread::spawn(move || {
        let opts = Options {
            delay_us: |_| std::thread::yield_now(),
            ..Options::default()
        };
        let dev = Dev::new_remote(sh2.remote(), sh2.io.clone(), opts).unwrap();
        let ep = dev.create_endpoint("rpmsg-echo", ADDR_ANY, ADDR_ANY).unwrap();
        loop {
            match dev.try_recv().unwrap() {
                Some(Received::Message(m)) => {
                    if m.data() == b"quit" {
                        return;
                    }
                    dev.send_to(ep, m.src(), m.data()).unwrap();
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
    let host = Dev::new_host(sh.host(), sh.io.clone(), sh.pool(), HostConfig::default(), opts)
        .unwrap();
    let ev = loop {
        if let Some(Received::NameService(ev)) = host.try_recv().unwrap() {
            break ev;
        }
        std::thread::yield_now();
    };
    let ep = host.create_endpoint("rpmsg-echo", ADDR_ANY, ev.addr).unwrap();

    const N: u32 = 100_000;
    let (mut sent, mut got) = (0u32, 0u32);
    let msg = |i: u32| {
        let len = 4 + (i as usize * 7) % 492;
        let mut v = vec![(i % 251) as u8; len];
        v[..4].copy_from_slice(&i.to_le_bytes());
        v
    };
    while got < N {
        while sent < N && sent - got < 8 {
            match host.try_send(ep, &msg(sent)) {
                Ok(_) => sent += 1,
                Err(Error::NoBuffer) => break,
                Err(e) => panic!("{e}"),
            }
        }
        if let Some(Received::Message(m)) = host.try_recv().unwrap() {
            assert_eq!(m.data(), &msg(got)[..], "message {got}");
            got += 1;
        }
    }
    host.send(ep, b"quit").unwrap();
    remote.join().unwrap();
}
