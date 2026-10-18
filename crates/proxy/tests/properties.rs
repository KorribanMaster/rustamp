// SPDX-License-Identifier: BSD-3-Clause
//! Property tests: the syscall encoding, and the host server under arbitrary
//! requests from the remote.
#![cfg(feature = "std")]

use std::path::Path;

use openamp_proxy::server::ProxyServer;
use openamp_proxy::wire::{
    CLOSE_SYSCALL_ID, OPEN_SYSCALL_ID, READ_SYSCALL_ID, SYSCALL_SIZE, Syscall, TERM_SYSCALL_ID,
    WRITE_SYSCALL_ID,
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

proptest! {
    #![proptest_config(config())]

    #[test]
    fn syscall_round_trips(id: u32, int_field1: i32, int_field2: i32, data_len: u32) {
        let s = Syscall { id, int_field1, int_field2, data_len };
        prop_assert_eq!(Syscall::from_bytes(&s.to_bytes()), Some(s));
    }

    #[test]
    fn syscall_needs_a_full_header(b in prop::collection::vec(any::<u8>(), 0..32)) {
        let s = Syscall::from_bytes(&b);
        prop_assert_eq!(s.is_some(), b.len() >= SYSCALL_SIZE);
        if let Some(s) = s {
            prop_assert_eq!(&s.to_bytes()[..], &b[..SYSCALL_SIZE]);
        }
    }
}

/// Path components a remote might try, including escapes.
fn path() -> impl Strategy<Value = Vec<u8>> {
    let comp = prop_oneof![
        Just(".."),
        Just("."),
        Just(""),
        Just("a"),
        Just("b"),
        Just("outside"),
        Just("canary"),
        Just("root"),
    ];
    (any::<bool>(), prop::collection::vec(comp, 0..6)).prop_map(|(abs, cs)| {
        let mut p = if abs { "/".to_string() } else { String::new() };
        p.push_str(&cs.join("/"));
        p.into_bytes()
    })
}

fn request() -> impl Strategy<Value = Vec<u8>> {
    let call = |id: u32, f1: i32, f2: i32, data: Vec<u8>| {
        let mut v = Syscall {
            id,
            int_field1: f1,
            int_field2: f2,
            data_len: data.len() as u32,
        }
        .to_bytes()
        .to_vec();
        v.extend_from_slice(&data);
        v
    };
    let fd = prop_oneof![0..8i32, any::<i32>()];
    prop_oneof![
        3 => (path(), any::<i32>(), any::<bool>()).prop_map(move |(mut p, flags, nul)| {
            if nul {
                p.push(0);
            }
            call(OPEN_SYSCALL_ID, flags, 0o600, p)
        }),
        2 => (fd.clone(), any::<i32>(), prop::collection::vec(any::<u8>(), 0..64))
            .prop_map(move |(fd, n, d)| call(WRITE_SYSCALL_ID, fd, n, d)),
        2 => (fd.clone(), any::<i32>()).prop_map(move |(fd, n)| call(READ_SYSCALL_ID, fd, n, vec![])),
        1 => fd.prop_map(move |fd| call(CLOSE_SYSCALL_ID, fd, 0, vec![])),
        1 => (any::<u32>(), any::<i32>(), any::<i32>(), any::<u32>(), prop::collection::vec(any::<u8>(), 0..32))
            .prop_map(|(id, a, b, len, d)| {
                let mut v = Syscall { id, int_field1: a, int_field2: b, data_len: len }.to_bytes().to_vec();
                v.extend_from_slice(&d);
                v
            }),
        1 => prop::collection::vec(any::<u8>(), 0..40),
    ]
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

proptest! {
    #![proptest_config(config())]

    /// Whatever the remote sends, the server neither panics nor touches
    /// anything outside its root.
    #[test]
    fn server_stays_inside_root(reqs in prop::collection::vec(request(), 1..40)) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CASE: AtomicU32 = AtomicU32::new(0);
        let top = std::env::temp_dir().join(format!(
            "openamp-proxy-prop-{}-{}",
            std::process::id(),
            CASE.fetch_add(1, Ordering::Relaxed)
        ));
        let root = top.join("root");
        let outside = top.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("canary"), b"untouched").unwrap();
        std::fs::write(top.join("canary"), b"untouched").unwrap();

        let mut srv = ProxyServer::new(&root).with_stdio(
            Box::new(std::io::empty()),
            Box::new(std::io::sink()),
            Box::new(std::io::sink()),
        );
        for r in &reqs {
            if let Some(reply) = srv.handle(r) {
                let h = Syscall::from_bytes(&reply).unwrap();
                prop_assert_eq!(reply.len(), SYSCALL_SIZE + h.data_len as usize);
                prop_assert_ne!(h.id, TERM_SYSCALL_ID);
            }
        }
        drop(srv);

        let top_names = names(&top);
        let outside_names = names(&outside);
        let canary_top = std::fs::read(top.join("canary")).unwrap();
        let canary_out = std::fs::read(outside.join("canary")).unwrap();
        std::fs::remove_dir_all(&top).unwrap();
        prop_assert_eq!(top_names, ["canary", "outside", "root"]);
        prop_assert_eq!(outside_names, ["canary"]);
        prop_assert_eq!(canary_top, b"untouched");
        prop_assert_eq!(canary_out, b"untouched");
    }
}
