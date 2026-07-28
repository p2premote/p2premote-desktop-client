use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_else(|_| env::consts::OS.to_string());
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("..").join("src-tauri").join("resources");
    let punch_library = resources_dir.join(punch_library_name(&target_os));
    println!("cargo:rustc-link-search=native={}", resources_dir.display());
    emit_link_flags(&target_os);
    println!("cargo:rerun-if-changed={}", punch_library.display());

    if !punch_library.exists() {
        println!(
            "cargo:warning={} not found; wgvpn binaries require it during build",
            punch_library.display()
        );
        return;
    }

    if requires_runtime_library_copy(&target_os) {
        let Some(profile_dir) = target_profile_dir() else {
            println!(
                "cargo:warning=failed to resolve target profile dir for p2premote punch library"
            );
            return;
        };
        copy_library(&punch_library, &profile_dir);
        copy_library(&punch_library, &profile_dir.join("deps"));
    }
}

fn emit_link_flags(target_os: &str) {
    if target_os == "linux" {
        println!("cargo:rustc-link-lib=static=p2premote-punch");
    }
}

fn requires_runtime_library_copy(target_os: &str) -> bool {
    target_os != "linux"
}

fn punch_library_name(target_os: &str) -> &'static str {
    match target_os {
        "windows" => "p2premote-punch.dll",
        "macos" => "libp2premote-punch.dylib",
        "linux" => "libp2premote-punch.a",
        _ => "libp2premote-punch.so",
    }
}

fn target_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    out_dir.ancestors().nth(3).map(Path::to_path_buf)
}

fn copy_library(source: &Path, dir: &Path) {
    if let Err(err) = fs::create_dir_all(dir) {
        println!("cargo:warning=failed to create {}: {}", dir.display(), err);
        return;
    }
    let target = dir.join(source.file_name().expect("punch library file name"));
    if let Err(err) = fs::copy(source, &target) {
        println!(
            "cargo:warning=failed to copy {} to {}: {}",
            source.display(),
            target.display(),
            err
        );
    }
}
