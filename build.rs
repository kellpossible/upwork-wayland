//! Compile the `LD_PRELOAD` capture shim (`shim/gdk_shim.rs`) into a cdylib and
//! expose its path to the crate as `UPWORK_GDK_SHIM` so `launcher.rs` can embed
//! it with `include_bytes!`.
//!
//! We invoke `rustc` directly rather than declaring the shim as an artifact
//! dependency: it's a single dependency-free source file, so a one-shot
//! `rustc --crate-type cdylib` keeps the build simple and avoids nested cargo.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let src = "shim/gdk_shim.rs";
    println!("cargo:rerun-if-changed={src}");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let so = PathBuf::from(&out_dir).join("libupwork_gdk_shim.so");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let status = Command::new(&rustc)
        .args(["--edition", "2024", "--crate-type", "cdylib"])
        .args(["-C", "opt-level=2"])
        .args(["-C", "strip=symbols"])
        .arg(src)
        .arg("-o")
        .arg(&so)
        .status()
        .expect("failed to invoke rustc to build the capture shim");
    assert!(status.success(), "building {src} as a cdylib failed");

    println!("cargo:rustc-env=UPWORK_GDK_SHIM={}", so.display());
}
