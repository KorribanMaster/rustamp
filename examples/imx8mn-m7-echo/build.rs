// SPDX-License-Identifier: BSD-3-Clause
//! Puts link.x on the linker search path.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("link.x", out.join("link.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=link.x");
}
