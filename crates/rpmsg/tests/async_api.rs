// SPDX-License-Identifier: BSD-3-Clause
//! The async add-on, driven by `pollster`, which sleeps until woken: a
//! missed wake-up hangs the test.
#![cfg(feature = "async")]

mod common;

use std::sync::Arc;

use common::Shared;
use pollster::block_on;
use rustamp_rpmsg::{ADDR_ANY, HostConfig, Options, Received, RpmsgVirtio};

type Dev = RpmsgVirtio<common::Sim>;

#[test]
fn recv_and_send_await() {
    let sh = Shared::new(0);
    let host = Arc::new(
        Dev::new_host(
            sh.host(),
            sh.io.clone(),
            sh.pool(),
            HostConfig::default(),
            Options::default(),
        )
        .unwrap(),
    );
    let remote =
        Arc::new(Dev::try_new_remote(sh.remote(), sh.io.clone(), Options::default()).unwrap());
    let r_ep = remote.create_endpoint("", 0x40, ADDR_ANY).unwrap();
    let h_ep = host.create_endpoint("", ADDR_ANY, 0x40).unwrap();

    // The "interrupt": another thread sends and then notifies the receiver.
    let h2 = host.clone();
    let r2 = remote.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        h2.send(h_ep, b"async hello").unwrap();
        r2.on_notify();
    });
    let got = block_on(async {
        loop {
            if let Received::Message(m) = remote.recv().await.unwrap() {
                return m.data().to_vec();
            }
        }
    });
    t.join().unwrap();
    assert_eq!(got, b"async hello");

    // Fill the host's transmit ring, then let the remote drain it while the
    // host awaits a buffer.
    for _ in 0..common::VRING_NUM {
        host.try_send(h_ep, b"fill").unwrap();
    }
    let host2 = host.clone();
    let sender = std::thread::spawn(move || {
        block_on(host2.send_async(h_ep, None, b"after wait")).unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(20));
    let mut seen = 0;
    while seen < common::VRING_NUM as usize + 1 {
        if let Some(Received::Message(m)) = remote.try_recv().unwrap() {
            seen += 1;
            if seen == common::VRING_NUM as usize + 1 {
                assert_eq!(m.data(), b"after wait");
            }
        }
        host.on_notify();
    }
    sender.join().unwrap();
    let _ = r_ep;
}
