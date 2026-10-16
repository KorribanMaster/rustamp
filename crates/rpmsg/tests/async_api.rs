// SPDX-License-Identifier: BSD-3-Clause
//! The async add-on, driven by a minimal thread-parking executor.
#![cfg(feature = "async")]

mod common;

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

use common::Shared;
use openamp_rpmsg::{ADDR_ANY, HostConfig, Options, Received, RpmsgVirtio};

struct Unpark(Thread);
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(f: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park_timeout(std::time::Duration::from_millis(100));
    }
}

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
    let remote = Dev::try_new_remote(sh.remote(), sh.io.clone(), Options::default()).unwrap();
    let r_ep = remote.create_endpoint("", 0x40, ADDR_ANY).unwrap();
    let h_ep = host.create_endpoint("", ADDR_ANY, 0x40).unwrap();

    // The "interrupt": another thread sends and then notifies the receiver.
    let h2 = host.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        h2.send(h_ep, b"async hello").unwrap();
    });
    let got = block_on(async {
        loop {
            // Poll-based wake-up stands in for on_notify() in this test.
            remote.on_notify();
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
