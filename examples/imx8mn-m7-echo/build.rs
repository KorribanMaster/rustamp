// SPDX-License-Identifier: BSD-3-Clause
//! Puts the memory layout on the linker search path as `memory.x`, which
//! cortex-m-rt's `link.x` includes: `memory-tcm.x` (TCM, real silicon) or
//! `memory-qemu.x` (DDR, QEMU) with `--features qemu-imx8mp`.
//!
//! No `memory.x` or `link.x` may sit in the crate root: the linker would
//! find it before this one, and the feature would silently do nothing.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let script = if env::var_os("CARGO_FEATURE_QEMU_IMX8MP").is_some() {
        "memory-qemu.x"
    } else {
        "memory-tcm.x"
    };
    fs::copy(script, out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory-tcm.x");
    println!("cargo:rerun-if-changed=memory-qemu.x");
}
