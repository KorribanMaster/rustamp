// SPDX-License-Identifier: BSD-3-Clause
//! Puts `rtu.x` on the linker search path and rejects conflicting boards.
use std::env;

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={dir}");
    println!("cargo:rerun-if-changed=rtu.x");
    if env::var_os("CARGO_FEATURE_ARRIA10").is_some()
        && env::var_os("CARGO_FEATURE_ZYNQ7000").is_some()
    {
        panic!("pick one board: --no-default-features --features arria10");
    }
}
