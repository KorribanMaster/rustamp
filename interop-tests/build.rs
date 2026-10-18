// SPDX-License-Identifier: BSD-3-Clause
//! Builds C libmetal and open-amp (Linux flavour) plus the C test peers.
//!
//! Sources come from `$OPENAMP_C_SRC` (containing `libmetal/` and
//! `open-amp/`), defaulting to `./c-src` as populated by
//! `fetch-c-sources.sh`. libmetal's Linux backend wants libsysfs; a stub
//! is built instead, since the tests never use the device API.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src = env::var("OPENAMP_C_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("c-src"));
    let (metal_src, oa_src) = (src.join("libmetal"), src.join("open-amp"));
    assert!(
        metal_src.is_dir() && oa_src.is_dir(),
        "C sources not found in {}; run interop-tests/fetch-c-sources.sh or set OPENAMP_C_SRC",
        src.display()
    );
    let stub = manifest.join("c/stub-sysfs");

    // Static libraries must be linked after their users, so the link
    // directives are emitted below, in order, rather than by `cc`.
    cc::Build::new()
        .file(stub.join("stub.c"))
        .include(&stub)
        .extra_warnings(true)
        .cargo_metadata(false)
        .compile("sysfs");

    let sysfs = |c: &mut cmake::Config| {
        c.profile("Release")
            .define("LIBSYSFS_INCLUDE_DIR", &stub)
            .define("LIBSYSFS_LIBRARY", out.join("libsysfs.a"))
            .define("WITH_SHARED_LIB", "OFF");
    };
    let mut metal = cmake::Config::new(&metal_src);
    sysfs(&mut metal);
    let metal = metal
        .out_dir(out.join("metal"))
        .define("WITH_TESTS", "OFF")
        .define("WITH_DOC", "OFF")
        .define("WITH_EXAMPLES", "OFF")
        .build();

    let mut oa = cmake::Config::new(&oa_src);
    sysfs(&mut oa);
    let oa = oa
        .out_dir(out.join("open-amp"))
        .define("CMAKE_INCLUDE_PATH", metal.join("include"))
        .define("CMAKE_LIBRARY_PATH", metal.join("lib"))
        .cflag(format!("-I{}", stub.display()))
        .define("WITH_PROXY", "ON")
        .build();

    cc::Build::new()
        .file(manifest.join("c/glue.c"))
        .include(metal.join("include"))
        .include(oa.join("include"))
        .include(&stub)
        // rpmsg_internal.h, for the private wire structures.
        .include(oa_src.join("lib/rpmsg"))
        .extra_warnings(true)
        .cargo_metadata(false)
        .compile("interop_glue");

    for dir in [out.clone(), oa.join("lib"), metal.join("lib")] {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    for l in ["interop_glue", "open_amp", "metal", "sysfs"] {
        println!("cargo:rustc-link-lib=static={l}");
    }
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=rt");
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OPENAMP_C_SRC");
}
