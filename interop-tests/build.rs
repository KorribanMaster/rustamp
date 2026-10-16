// SPDX-License-Identifier: BSD-3-Clause
//! Builds C libmetal and open-amp (Linux flavour) plus the C test peers.
//!
//! Sources come from `$OPENAMP_C_SRC` (containing `libmetal/` and
//! `open-amp/`), defaulting to `./c-src` as populated by
//! `fetch-c-sources.sh`. libmetal's Linux backend wants libsysfs; a stub
//! is built instead, since the tests never use the device API.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {cmd:?}: {e}"));
    assert!(status.success(), "command failed: {cmd:?}");
}

fn cc() -> String {
    env::var("CC").unwrap_or_else(|_| "cc".into())
}

fn static_lib(out: &Path, name: &str, src: &Path, includes: &[&Path]) {
    let obj = out.join(format!("{name}.o"));
    let mut c = Command::new(cc());
    c.args(["-c", "-O2", "-fPIC", "-Wall", "-Wextra"]);
    for i in includes {
        c.arg("-I").arg(i);
    }
    run(c.arg(src).arg("-o").arg(&obj));
    let lib = out.join(format!("lib{name}.a"));
    let _ = std::fs::remove_file(&lib);
    run(Command::new("ar").arg("rcs").arg(&lib).arg(&obj));
}

fn cmake(src: &Path, build: &Path, defs: &[String]) {
    let mut c = Command::new("cmake");
    c.arg("-S").arg(src).arg("-B").arg(build);
    c.arg("-DCMAKE_BUILD_TYPE=Release");
    for d in defs {
        c.arg(format!("-D{d}"));
    }
    run(&mut c);
    run(Command::new("cmake")
        .arg("--build")
        .arg(build)
        .args(["--target", "install", "-j", "4"]));
}

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
    let inst = out.join("inst");

    static_lib(&out, "sysfs", &stub.join("stub.c"), &[&stub]);
    let sysfs = [
        format!("LIBSYSFS_INCLUDE_DIR={}", stub.display()),
        format!("LIBSYSFS_LIBRARY={}", out.join("libsysfs.a").display()),
        format!("CMAKE_INSTALL_PREFIX={}", inst.display()),
        "WITH_SHARED_LIB=OFF".into(),
    ];
    let mut defs = sysfs.to_vec();
    defs.extend(["WITH_TESTS=OFF", "WITH_DOC=OFF", "WITH_EXAMPLES=OFF"].map(String::from));
    cmake(&metal_src, &out.join("build-metal"), &defs);

    let mut defs = sysfs.to_vec();
    defs.extend([
        format!("CMAKE_INCLUDE_PATH={}", inst.join("include").display()),
        format!("CMAKE_LIBRARY_PATH={}", inst.join("lib").display()),
        format!("CMAKE_C_FLAGS=-I{}", stub.display()),
        "WITH_PROXY=ON".into(),
    ]);
    cmake(&oa_src, &out.join("build-oa"), &defs);

    static_lib(
        &out,
        "interop_glue",
        &manifest.join("c/glue.c"),
        &[&inst.join("include"), &stub],
    );

    println!("cargo:rustc-link-search=native={}", out.display());
    println!(
        "cargo:rustc-link-search=native={}",
        inst.join("lib").display()
    );
    for l in ["interop_glue", "open_amp", "metal", "sysfs"] {
        println!("cargo:rustc-link-lib=static={l}");
    }
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=rt");
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OPENAMP_C_SRC");
}
