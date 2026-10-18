// SPDX-License-Identifier: BSD-3-Clause
//! Remote proxy client and host server talking over rpmsg.
#![cfg(feature = "std")]

#[path = "../../rpmsg/tests/common/mod.rs"]
mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::Shared;
use rustamp_proxy::server::ProxyServer;
use rustamp_proxy::{Error, PROXY_ENDPOINT_NAME, ProxyClient, RpmsgChannel, flags};
use rustamp_rpmsg::{ADDR_ANY, HostConfig, Options, Received, RpmsgVirtio, VIRTIO_RPMSG_F_NS};

type Dev = RpmsgVirtio<common::Sim>;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn printf_and_files_through_the_proxy() {
    let sh = Shared::new(1 << VIRTIO_RPMSG_F_NS);
    let opts = Options {
        delay_us: |_| std::thread::yield_now(),
        ..Options::default()
    };
    let host = Arc::new(
        Dev::new_host(
            sh.host(),
            sh.io.clone(),
            sh.pool(),
            HostConfig::default(),
            opts,
        )
        .unwrap(),
    );
    let remote = Dev::try_new_remote(sh.remote(), sh.io.clone(), opts).unwrap();

    // Remote announces the proxy service; host binds to it.
    let r_ep = remote
        .create_endpoint(PROXY_ENDPOINT_NAME, ADDR_ANY, ADDR_ANY)
        .unwrap();
    let Some(Received::NameService(ev)) = host.try_recv().unwrap() else {
        panic!("no announcement")
    };
    let h_ep = host
        .create_endpoint(PROXY_ENDPOINT_NAME, ADDR_ANY, ev.addr)
        .unwrap();
    // As in the C demo, the host speaks first so the remote learns its address.
    host.send(h_ep, b"").unwrap();
    {
        let Some(Received::Message(_)) = remote.try_recv().unwrap() else {
            panic!()
        };
    }

    // Host: serve until told to stop.
    let root = std::env::temp_dir().join(format!("rustamp-proxy-rpmsg-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let out = Capture::default();
    let mut srv = ProxyServer::new(&root).with_stdio(
        Box::new(std::io::empty()),
        Box::new(out.clone()),
        Box::new(std::io::sink()),
    );
    let host2 = host.clone();
    let server = std::thread::spawn(move || {
        loop {
            match host2.try_recv().unwrap() {
                Some(Received::Message(m)) => {
                    if m.data() == b"stop" {
                        return;
                    }
                    if let Some(reply) = srv.handle(m.data()) {
                        host2.send(h_ep, &reply).unwrap();
                    }
                }
                Some(_) => {}
                None => std::thread::yield_now(),
            }
        }
    });

    // Remote: printf and a file round trip.
    let mut client = ProxyClient::new(RpmsgChannel::new(&remote, r_ep, std::thread::yield_now));
    for i in 0..3 {
        core::fmt::Write::write_fmt(&mut client.stdout(), format_args!("tick {i}\n")).unwrap();
    }
    let fd = client
        .open("log.bin", flags::O_RDWR | flags::O_CREAT, 0o600)
        .unwrap();
    let blob: Vec<u8> = (0..2000u32).map(|i| i as u8).collect();
    client.write_all(fd, &blob).unwrap();
    client.close(fd).unwrap();
    assert!(matches!(client.close(fd), Err(Error::Remote(-9))));
    let fd = client.open("log.bin", flags::O_RDONLY, 0).unwrap();
    let mut back = Vec::new();
    let mut buf = [0u8; 300];
    loop {
        let n = client.read(fd, &mut buf).unwrap();
        if n == 0 {
            break;
        }
        back.extend_from_slice(&buf[..n]);
    }
    assert_eq!(back, blob);

    // The same through embedded-io.
    #[cfg(feature = "embedded-io")]
    {
        use embedded_io::{Error as _, ErrorKind, Read, Write};
        let mut f = client
            .open_file("eio.bin", flags::O_RDWR | flags::O_CREAT, 0o600)
            .unwrap();
        f.write_all(&blob).unwrap();
        f.flush().unwrap();
        drop(f); // closes the fd
        let mut f = client.open_file("eio.bin", flags::O_RDONLY, 0).unwrap();
        let mut back = Vec::new();
        loop {
            let n = f.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            back.extend_from_slice(&buf[..n]);
        }
        assert_eq!(back, blob);
        f.close().unwrap();
        let e = client
            .open_file("missing.bin", flags::O_RDONLY, 0)
            .unwrap_err();
        assert_eq!(
            rustamp_proxy::FileError::from(e).kind(),
            ErrorKind::NotFound
        );
    }

    remote.send(r_ep, b"stop").unwrap();
    server.join().unwrap();
    assert_eq!(&out.0.lock().unwrap()[..], b"tick 0\ntick 1\ntick 2\n");
    std::fs::remove_dir_all(root).unwrap();
}
