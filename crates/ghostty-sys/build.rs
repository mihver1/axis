use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"));
    let workspace_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("ghostty-sys should live in crates/ghostty-sys");
    let vendor_dir = workspace_dir.join("vendor/ghostty");

    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("build.rs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        vendor_dir.join("build.zig").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        vendor_dir.join("build.zig.zon").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        vendor_dir.join("include").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        vendor_dir.join("src").display()
    );
    println!("cargo:rerun-if-env-changed=ZIG");

    if !vendor_dir.join("build.zig").exists() {
        panic!(
            "vendor/ghostty is missing. Expected Ghostty sources at {}",
            vendor_dir.display()
        );
    }

    let zig = find_zig();
    let status = Command::new(&zig)
        .current_dir(&vendor_dir)
        .arg("build")
        .arg("-Demit-lib-vt")
        .status()
        .expect("failed to invoke zig build for libghostty-vt");

    if !status.success() {
        panic!("zig build -Demit-lib-vt failed with status {status}");
    }

    let lib_dir = vendor_dir.join("zig-out/lib");
    let dylib = lib_dir.join(dylib_name());
    if !dylib.exists() {
        panic!("expected built libghostty-vt at {}", dylib.display());
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=ghostty-vt");

    if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    }

    if let Some(runtime_name) = runtime_library_name() {
        let runtime_src = lib_dir.join(runtime_name);
        if runtime_src.exists() {
            let out_dir =
                PathBuf::from(env::var_os("OUT_DIR").expect("missing OUT_DIR for build script"));
            let profile_dir = out_dir
                .ancestors()
                .nth(3)
                .expect("OUT_DIR should live under target/<profile>/build/.../out");
            copy_runtime_library(&runtime_src, &profile_dir.join(runtime_name));
            copy_runtime_library(&runtime_src, &profile_dir.join("deps").join(runtime_name));
        }
    }

    println!(
        "cargo:rustc-env=GHOSTTY_VENDOR_DIR={}",
        vendor_dir.display()
    );
    println!("cargo:rustc-env=GHOSTTY_VT_LIB_DIR={}", lib_dir.display());
}

fn find_zig() -> PathBuf {
    if let Some(path) = env::var_os("ZIG") {
        return PathBuf::from(path);
    }

    for candidate in ["/opt/homebrew/bin/zig", "/usr/local/bin/zig", "zig"] {
        let path = PathBuf::from(candidate);
        if path.is_absolute() {
            if path.exists() {
                return path;
            }
        } else {
            return path;
        }
    }

    panic!("zig not found. Install zig or set the ZIG environment variable");
}

fn dylib_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libghostty-vt.dylib"
    } else if cfg!(target_os = "windows") {
        "ghostty-vt.dll"
    } else {
        "libghostty-vt.so"
    }
}

fn runtime_library_name() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("libghostty-vt.dylib")
    } else if cfg!(target_os = "windows") {
        Some("ghostty-vt.dll")
    } else if cfg!(target_os = "linux") {
        Some("libghostty-vt.so")
    } else {
        None
    }
}

fn copy_runtime_library(src: &Path, dest: &Path) {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).expect("failed to create runtime library directory");
    }

    if dest.exists() {
        fs::remove_file(dest).expect("failed to replace existing runtime library copy");
    }

    fs::copy(src, dest).unwrap_or_else(|error| {
        panic!(
            "failed to copy runtime library from {} to {}: {}",
            src.display(),
            dest.display(),
            error
        )
    });
}
