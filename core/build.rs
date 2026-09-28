use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is not set");
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    println!("cargo:rerun-if-env-changed=P2PREMOTE_PUNCH_LIB_DIR");
    let resources_dir = env::var_os("P2PREMOTE_PUNCH_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("..").join("src-tauri").join("resources"));
    let punch_library = resources_dir.join(punch_library_name(&target_os));
    println!("cargo:rustc-link-search=native={}", resources_dir.display());
    println!("cargo:rerun-if-changed={}", punch_library.display());

    // Punch/Exchange is compiled in from source on every platform via the
    // p2premote-punch path dependency. Windows/macOS additionally load the
    // userspace WireGuard data plane library (p2premote-wg-ffi, wgonly) at
    // build time for linking; Linux needs no prebuilt library.
    if target_os != "linux" && !punch_library.exists() {
        panic!(
            "{} not found; wgvpn binaries require it during build",
            punch_library.display()
        );
    }

    if requires_runtime_library_copy(&target_os) {
        let profile_dir = target_profile_dir();
        copy_library(&punch_library, &profile_dir);
        copy_library(&punch_library, &profile_dir.join("deps"));
    }
}

fn requires_runtime_library_copy(target_os: &str) -> bool {
    target_os != "linux"
}

fn punch_library_name(target_os: &str) -> &'static str {
    match target_os {
        "windows" => "p2premote-wg.dll",
        "macos" => "libp2premote-wg.dylib",
        "linux" => "libp2premote-punch.a",
        other => panic!("unsupported target OS: {other}"),
    }
}

fn target_profile_dir() -> PathBuf {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is not set"));
    out_dir
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            panic!(
                "failed to resolve target profile dir from {}",
                out_dir.display()
            )
        })
}

fn copy_library(source: &Path, dir: &Path) {
    fs::create_dir_all(dir)
        .unwrap_or_else(|err| panic!("failed to create {}: {}", dir.display(), err));
    let target = dir.join(source.file_name().expect("punch library file name"));
    fs::copy(source, &target).unwrap_or_else(|err| {
        panic!(
            "failed to copy {} to {}: {}",
            source.display(),
            target.display(),
            err
        )
    });
}
