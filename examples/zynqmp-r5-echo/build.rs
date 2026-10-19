// SPDX-License-Identifier: BSD-3-Clause
//! Puts `memory.x` (included by aarch32-rt's `link.x`) on the linker path.
use std::env;

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={dir}");
    println!("cargo:rerun-if-changed=memory.x");
}
