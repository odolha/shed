//! GPUI's X11 backend links `libxkbcommon-x11`. Distros ship the unversioned
//! `libxkbcommon-x11.so` only in the -dev package; when it is missing but the
//! runtime library is present, link against the runtime library instead.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    const DIRS: [&str; 4] = ["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/usr/lib", "/usr/lib/aarch64-linux-gnu"];
    if DIRS.iter().any(|d| Path::new(d).join("libxkbcommon-x11.so").exists()) {
        return;
    }
    let Some(runtime) = DIRS.iter().map(|d| Path::new(d).join("libxkbcommon-x11.so.0")).find(|p| p.exists()) else {
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("xkb-link");
    std::fs::create_dir_all(&out).unwrap();
    let link = out.join("libxkbcommon-x11.so");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&runtime, &link).unwrap();
    println!("cargo:rustc-link-search=native={}", out.display());
    println!(
        "cargo:warning=libxkbcommon-x11-dev is not installed; linking {} directly",
        runtime.display()
    );
}
