use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is not set");
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    println!("cargo:rerun-if-env-changed=P2PREMOTE_PUNCH_LIB_DIR");
    println!("cargo:rerun-if-env-changed=P2PREMOTE_WG_FFI_DIR");
    // core/src 用 option_env! 编译期固化客户端版本号；不声明该项时，持久
    // target 缓存（本地 rust-cache、CI rust-cache）下仅变化该 env 不会触发
    // 重编译，会产出携带旧版本号的二进制
    println!("cargo:rerun-if-env-changed=P2PREMOTE_CLIENT_VERSION");
    // Package target also determines the device type reported at registration.
    // Keep cached builds from retaining another package's baked-in type.
    println!("cargo:rerun-if-env-changed=P2PREMOTE_RELEASE_TARGET");
    let resources_dir = env::var_os("P2PREMOTE_PUNCH_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("..").join("src-tauri").join("resources"));
    let punch_library = resources_dir.join(punch_library_name(&target_os));
    if target_os == "windows" {
        if let Some(source_dir) = env::var_os("P2PREMOTE_WG_FFI_DIR") {
            build_windows_wg_library(Path::new(&source_dir), &punch_library);
        }
    }
    if target_os != "windows" {
        println!("cargo:rustc-link-search=native={}", resources_dir.display());
    }
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
        if target_os == "windows" {
            generate_windows_import_library(&punch_library);
        }
        let profile_dir = target_profile_dir();
        copy_library(&punch_library, &profile_dir);
        copy_library(&punch_library, &profile_dir.join("deps"));
    }
}

fn build_windows_wg_library(source: &Path, dll: &Path) {
    use std::process::Command;
    assert!(
        source.is_dir(),
        "WG source directory is missing: {}",
        source.display()
    );
    println!("cargo:rerun-if-changed={}", source.display());
    fs::create_dir_all(dll.parent().expect("WG DLL has no parent"))
        .expect("failed to create WG resource directory");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "amd64",
        Ok("aarch64") => "arm64",
        other => panic!("unsupported Windows WG architecture: {:?}", other),
    };
    let output = Command::new("go")
        .current_dir(source)
        .env("GOTOOLCHAIN", "go1.20.14")
        .env("GOOS", "windows")
        .env("GOARCH", arch)
        .args([
            "build",
            "-tags=wgonly",
            "-mod=mod",
            "-buildmode=c-shared",
            "-ldflags",
            "-s -w",
            "-o",
        ])
        .arg(dll)
        .arg("./punchffi")
        .output()
        .expect("failed to invoke WG go build");
    assert!(
        output.status.success(),
        "WG DLL build failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let header = dll.with_extension("h");
    if header.is_file() {
        fs::remove_file(&header).expect("failed to remove generated WG header");
    }
}

fn generate_windows_import_library(dll: &Path) {
    let target = env::var("TARGET").expect("TARGET is not set");
    let machine = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "X64",
        Ok("aarch64") => "ARM64",
        other => panic!("unsupported Windows WG architecture: {:?}", other),
    };
    let dumpbin = cc::windows_registry::find_tool(&target, "dumpbin.exe")
        .expect("MSVC dumpbin.exe is required to read the actual WG DLL exports");
    let output = dumpbin
        .to_command()
        .arg("/nologo")
        .arg("/exports")
        .arg(dll)
        .output()
        .expect("failed to run dumpbin.exe");
    assert!(
        output.status.success(),
        "dumpbin failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Export rows contain: ordinal, hexadecimal hint, hexadecimal RVA, name.
    // Never derive this list from Rust declarations: that would hide missing exports.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let exports: Vec<&str> = stdout
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() >= 4
                && fields[0].parse::<u32>().is_ok()
                && u32::from_str_radix(fields[1], 16).is_ok()
                && u32::from_str_radix(fields[2], 16).is_ok()
                && fields[3]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                Some(fields[3])
            } else {
                None
            }
        })
        .collect();
    assert!(
        !exports.is_empty(),
        "no named exports found in {}",
        dll.display()
    );
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is not set"));
    let def = out_dir.join("p2premote-wg.def");
    let lib = out_dir.join("p2premote-wg.lib");
    fs::write(
        &def,
        format!(
            "LIBRARY p2premote-wg.dll\nEXPORTS\n{}\n",
            exports.join("\n")
        ),
    )
    .expect("failed to write WG export definition");
    let librarian = cc::windows_registry::find_tool(&target, "lib.exe")
        .expect("MSVC lib.exe is required to generate the WG import library");
    let output = librarian
        .to_command()
        .arg("/nologo")
        .arg(format!("/machine:{}", machine))
        .arg(format!("/def:{}", def.display()))
        .arg(format!("/out:{}", lib.display()))
        .output()
        .expect("failed to run lib.exe");
    assert!(
        output.status.success(),
        "WG import library generation failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("cargo:rustc-link-search=native={}", out_dir.display());
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
