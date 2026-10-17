// SPDX-License-Identifier: BSD-3-Clause
//! Puts the linker script on the search path as `link.x`: `tcm.x` (TCM,
//! real silicon) or `qemu-ddr.x` (DDR, QEMU) with `--features qemu-imx8mp`.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let script = if env::var_os("CARGO_FEATURE_QEMU_IMX8MP").is_some() {
        "qemu-ddr.x"
    } else {
        "tcm.x"
    };
    fs::copy(script, out.join("link.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=tcm.x");
    println!("cargo:rerun-if-changed=qemu-ddr.x");
}
