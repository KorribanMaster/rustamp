// SPDX-License-Identifier: BSD-3-Clause
//! The wire structures have the size and field offsets the C headers give
//! them.

use core::mem::{offset_of, size_of};
use std::collections::BTreeMap;
use std::ffi::CStr;

use openamp_interop_tests::{c_layout_count, c_layout_name, c_layout_value};
use openamp_remoteproc::rsc::{
    FwRscCarveout, FwRscTrace, FwRscVdev, FwRscVdevVring, RscTableHeader,
};
use openamp_rpmsg::wire::{HDR_SIZE, Header, NS_MSG_SIZE, NsMsg};
use openamp_rpmsg::{NAME_SIZE, Name};

fn c_layout() -> BTreeMap<String, usize> {
    // SAFETY: plain functions over a static table; names are NUL-terminated
    // static strings.
    unsafe {
        (0..c_layout_count())
            .map(|i| {
                let name = CStr::from_ptr(c_layout_name(i)).to_str().unwrap();
                (name.to_string(), c_layout_value(i))
            })
            .collect()
    }
}

/// Byte offset of the field whose bytes in `bytes` are all `marker`.
fn find(bytes: &[u8], marker: u8) -> usize {
    bytes.iter().position(|&b| b == marker).unwrap()
}

#[test]
fn layouts_match_the_c_headers() {
    let mut rust = BTreeMap::new();
    let mut put = |k: &str, v: usize| {
        rust.insert(k.to_string(), v);
    };

    put("resource_table", size_of::<RscTableHeader<0>>());
    put("resource_table.ver", offset_of!(RscTableHeader<0>, ver));
    put("resource_table.num", offset_of!(RscTableHeader<0>, num));
    put(
        "resource_table.offset",
        offset_of!(RscTableHeader<0>, offset),
    );

    put("fw_rsc_carveout", size_of::<FwRscCarveout>());
    put("fw_rsc_carveout.type", offset_of!(FwRscCarveout, rsc_type));
    put("fw_rsc_carveout.da", offset_of!(FwRscCarveout, da));
    put("fw_rsc_carveout.pa", offset_of!(FwRscCarveout, pa));
    put("fw_rsc_carveout.len", offset_of!(FwRscCarveout, len));
    put("fw_rsc_carveout.flags", offset_of!(FwRscCarveout, flags));
    put("fw_rsc_carveout.name", offset_of!(FwRscCarveout, name));

    put("fw_rsc_trace", size_of::<FwRscTrace>());
    put("fw_rsc_trace.type", offset_of!(FwRscTrace, rsc_type));
    put("fw_rsc_trace.da", offset_of!(FwRscTrace, da));
    put("fw_rsc_trace.len", offset_of!(FwRscTrace, len));
    put("fw_rsc_trace.name", offset_of!(FwRscTrace, name));

    type Vdev = FwRscVdev<0>;
    put("fw_rsc_vdev", size_of::<Vdev>());
    put("fw_rsc_vdev.type", offset_of!(Vdev, rsc_type));
    put("fw_rsc_vdev.id", offset_of!(Vdev, id));
    put("fw_rsc_vdev.notifyid", offset_of!(Vdev, notifyid));
    put("fw_rsc_vdev.dfeatures", offset_of!(Vdev, dfeatures));
    put("fw_rsc_vdev.gfeatures", offset_of!(Vdev, gfeatures));
    put("fw_rsc_vdev.config_len", offset_of!(Vdev, config_len));
    put("fw_rsc_vdev.status", offset_of!(Vdev, status));
    put("fw_rsc_vdev.num_of_vrings", offset_of!(Vdev, num_of_vrings));
    put("fw_rsc_vdev.vring", offset_of!(Vdev, vring));

    put("fw_rsc_vdev_vring", size_of::<FwRscVdevVring>());
    put("fw_rsc_vdev_vring.da", offset_of!(FwRscVdevVring, da));
    put("fw_rsc_vdev_vring.align", offset_of!(FwRscVdevVring, align));
    put("fw_rsc_vdev_vring.num", offset_of!(FwRscVdevVring, num));
    put(
        "fw_rsc_vdev_vring.notifyid",
        offset_of!(FwRscVdevVring, notifyid),
    );
    // `pa` in Linux, `reserved` in open-amp.
    put("fw_rsc_vdev_vring.reserved", offset_of!(FwRscVdevVring, pa));

    // The rpmsg structures are encoded by hand: locate each field in the
    // encoding by giving it a distinct byte value.
    let h = Header {
        src: 0x0101_0101,
        dst: 0x0202_0202,
        reserved: 0x0303_0303,
        len: 0x0404,
        flags: 0x0505,
    }
    .to_bytes();
    put("rpmsg_hdr", HDR_SIZE);
    for (i, f) in ["src", "dst", "reserved", "len", "flags"]
        .iter()
        .enumerate()
    {
        put(&format!("rpmsg_hdr.{f}"), find(&h, i as u8 + 1));
    }
    let ns = NsMsg {
        name: Name::new("\u{6}").unwrap(),
        addr: 0x0707_0707,
        flags: 0x0808_0808,
    }
    .to_bytes();
    put("rpmsg_ns_msg", NS_MSG_SIZE);
    put("rpmsg_ns_msg.name", find(&ns, 6));
    put("rpmsg_ns_msg.addr", find(&ns, 7));
    put("rpmsg_ns_msg.flags", find(&ns, 8));
    assert_eq!(find(&ns, 7) - find(&ns, 6), NAME_SIZE);

    assert_eq!(rust, c_layout());
}
