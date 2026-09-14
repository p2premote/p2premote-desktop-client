use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../service/src");
    println!("cargo:rerun-if-changed=../service/Cargo.toml");
    println!("cargo:rerun-if-changed=../notifier/src");
    println!("cargo:rerun-if-changed=../notifier/Cargo.toml");
    println!("cargo:rerun-if-changed=../core/src");
    println!("cargo:rerun-if-changed=../core/Cargo.toml");
    println!("cargo:rerun-if-env-changed=P2PREMOTE_PUNCH_DIR");
    println!("cargo:rerun-if-env-changed=P2PREMOTE_PREBUILT_RESOURCES");
    println!("cargo:rerun-if-env-changed=RUSTDESK_TINY_ARTIFACT_DIR");

    emit_macos_rpath();

    if env::var_os("P2PREMOTE_PREBUILT_RESOURCES").is_some() {
        validate_prebuilt_resources();
    } else {
        build_punch_library_into_resources();
        copy_service_into_resources();
        copy_cli_into_resources();
        copy_notifier_into_resources();
        copy_desktop_engine_into_resources();
    }
    tauri_build::build()
}

fn emit_macos_rpath() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Resources/resources");
    }
}

fn validate_prebuilt_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("resources");
    let (target_os, _) = parse_target();
    let mut required = vec![
        main_binary_name("p2premote-service", &target_os),
        main_binary_name("p2premote-cli", &target_os),
    ];
    if target_os != "linux" {
        required.push(punch_library_name(&target_os));
    }
    if target_os == "windows" {
        required.push(main_binary_name("p2premote-notifier", &target_os));
        for name in ["RustDeskTiny/RustDeskTiny.exe"] {
            let path = resources_dir.join(name);
            if !path.is_file() {
                panic!("required prebuilt resource is missing: {}", path.display());
            }
        }
    }
    for name in required {
        let path = resources_dir.join(&name);
        if !path.is_file() {
            panic!("required prebuilt resource is missing: {}", path.display());
        }
    }
}

fn main_binary_name(base: &str, target_os: &str) -> String {
    if target_os == "windows" {
        format!("{}.exe", base)
    } else {
        base.to_string()
    }
}

fn punch_library_name(target_os: &str) -> String {
    if target_os == "windows" {
        "p2premote-wg.dll".to_string()
    } else if target_os == "macos" {
        "libp2premote-punch.dylib".to_string()
    } else if target_os == "linux" {
        "libp2premote-punch.a".to_string()
    } else {
        panic!("unsupported target OS: {target_os}")
    }
}

fn parse_target() -> (String, String) {
    let target = env::var("TARGET").expect("TARGET is not set");
    if target.trim().is_empty() {
        panic!("TARGET is empty");
    }
    let target_os = match env::var("CARGO_CFG_TARGET_OS")
        .expect("CARGO_CFG_TARGET_OS is not set")
        .as_str()
    {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        other => panic!("unsupported target OS: {other} (target {target})"),
    };
    let target_arch = match env::var("CARGO_CFG_TARGET_ARCH")
        .expect("CARGO_CFG_TARGET_ARCH is not set")
        .as_str()
    {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => panic!("unsupported target architecture: {other} (target {target})"),
    };

    (target_os.to_string(), target_arch.to_string())
}

fn client_version() -> String {
    let version =
        env::var("P2PREMOTE_CLIENT_VERSION").expect("P2PREMOTE_CLIENT_VERSION is not set");
    if version.trim().is_empty() {
        panic!("P2PREMOTE_CLIENT_VERSION is empty");
    }
    version
}

fn build_punch_library_into_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let (target_os, _) = parse_target();
    // Linux links Rust Punch as a normal crate dependency and ships no punch
    // archive. Its WireGuard fallback is built separately below by the Linux
    // packaging scripts.
    if target_os == "linux" {
        return;
    }
    let source_env = if target_os == "windows" {
        "P2PREMOTE_WG_FFI_DIR"
    } else {
        "P2PREMOTE_PUNCH_DIR"
    };
    let source_dir = PathBuf::from(
        env::var_os(source_env).unwrap_or_else(|| panic!("{} is not set", source_env)),
    );
    if !source_dir.is_dir() {
        panic!(
            "WG/Punch source directory not found: {}",
            source_dir.display()
        );
    }
    println!("cargo:rerun-if-changed={}", source_dir.display());
    let resources_dir = manifest_dir.join("resources");
    let (target_os, target_arch) = parse_target();
    let library_name = punch_library_name(&target_os);
    let target = resources_dir.join(&library_name);

    fs::create_dir_all(&resources_dir).unwrap_or_else(|err| {
        panic!(
            "failed to create resources dir {}: {}",
            resources_dir.display(),
            err
        )
    });

    let mut command = Command::new("go");
    command.arg("build");
    if target_os == "linux" {
        command.arg("-buildmode=c-archive");
    } else {
        command.arg("-buildmode=c-shared");
    }
    // Windows Punch/Exchange now run in Rust. Build only the WireGuard data
    // plane with the Go 1.20-compatible module so the resulting DLL remains
    // runnable on Windows 7. macOS keeps the legacy combined library until
    // its WG implementation is migrated separately.
    if target_os == "windows" {
        command.arg("-tags").arg("wgonly");
        command.arg("-mod=mod");
        command.env("GOTOOLCHAIN", "go1.20.14");
    }
    command
        .arg("-ldflags")
        .arg("-s -w")
        .arg("-o")
        .arg(&target)
        .arg("./punchffi");
    command.current_dir(&source_dir);
    let go_os = if target_os == "macos" {
        "darwin"
    } else {
        &target_os
    };
    command.env("GOOS", go_os);
    command.env("GOARCH", &target_arch);

    let output = command
        .output()
        .unwrap_or_else(|err| panic!("failed to invoke go build for {}: {}", library_name, err));
    if !output.status.success() {
        panic!(
            "failed to build {}: {}",
            library_name,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if target_os == "macos" {
        let output = Command::new("install_name_tool")
            .args(["-id", "@rpath/libp2premote-punch.dylib"])
            .arg(&target)
            .output()
            .unwrap_or_else(|err| panic!("failed to invoke install_name_tool: {}", err));
        if !output.status.success() {
            panic!(
                "failed to set the macOS dylib install name: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    if target_os != "linux" {
        ensure_executable(&target);
    }
    remove_generated_c_header(&target);
}

fn remove_generated_c_header(library_path: &Path) {
    let header_path = library_path.with_extension("h");
    if header_path.exists() {
        fs::remove_file(&header_path).unwrap_or_else(|err| {
            panic!(
                "failed to remove generated header {}: {}",
                header_path.display(),
                err
            )
        });
    }
}

fn nested_target_dir(manifest_dir: &Path, name: &str) -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("target"))
        .join(name)
}

fn copy_service_into_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("resources");
    let profile = env::var("PROFILE").expect("PROFILE is not set");
    let service_manifest = manifest_dir.join("../service/Cargo.toml");
    let cargo = env::var("CARGO").expect("CARGO is not set");
    let target_triple = env::var("TARGET").expect("TARGET is not set");
    let target_dir = nested_target_dir(&manifest_dir, "service-build");
    let (target_os, _) = parse_target();
    let binary_name = main_binary_name("p2premote-service", &target_os);
    let source = resolve_built_service_path(&target_dir, &target_triple, &profile, &binary_name);
    let target = resources_dir.join(&binary_name);

    let mut command = Command::new(cargo);
    command.arg("build");
    command.arg("--manifest-path");
    command.arg(&service_manifest);
    command.arg("--bin");
    command.arg("p2premote-service");
    if profile.eq_ignore_ascii_case("release") {
        command.arg("--release");
    }
    command.arg("--target").arg(&target_triple);
    command.env("CARGO_TARGET_DIR", &target_dir);
    command.env("P2PREMOTE_CLIENT_VERSION", client_version());

    let output = command
        .output()
        .unwrap_or_else(|err| panic!("failed to invoke cargo build for {}: {}", binary_name, err));
    if !output.status.success() {
        panic!(
            "failed to build {}: {}",
            binary_name,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    copy_built_file(&source, &target, &resources_dir, &binary_name);
}

fn copy_cli_into_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("resources");
    let profile = env::var("PROFILE").expect("PROFILE is not set");
    let cli_manifest = manifest_dir.join("../cli/Cargo.toml");
    let cargo = env::var("CARGO").expect("CARGO is not set");
    let target_triple = env::var("TARGET").expect("TARGET is not set");
    let target_dir = nested_target_dir(&manifest_dir, "cli-build");
    let (target_os, _) = parse_target();
    let binary_name = main_binary_name("p2premote-cli", &target_os);
    let source = resolve_built_service_path(&target_dir, &target_triple, &profile, &binary_name);
    let target = resources_dir.join(&binary_name);

    let mut command = Command::new(cargo);
    command.arg("build");
    command.arg("--manifest-path");
    command.arg(&cli_manifest);
    command.arg("--bin");
    command.arg("p2premote-cli");
    if profile.eq_ignore_ascii_case("release") {
        command.arg("--release");
    }
    command.arg("--target").arg(&target_triple);
    command.env("CARGO_TARGET_DIR", &target_dir);
    command.env("P2PREMOTE_CLIENT_VERSION", client_version());

    let output = command
        .output()
        .unwrap_or_else(|err| panic!("failed to invoke cargo build for {}: {}", binary_name, err));
    if !output.status.success() {
        panic!(
            "failed to build {}: {}",
            binary_name,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    copy_built_file(&source, &target, &resources_dir, &binary_name);
}

fn copy_notifier_into_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("resources");
    let profile = env::var("PROFILE").expect("PROFILE is not set");
    let cargo = env::var("CARGO").expect("CARGO is not set");
    let target_triple = env::var("TARGET").expect("TARGET is not set");
    let target_dir = nested_target_dir(&manifest_dir, "notifier-build");
    let (target_os, _) = parse_target();
    let binary_name = main_binary_name("p2premote-notifier", &target_os);
    let source = resolve_built_service_path(&target_dir, &target_triple, &profile, &binary_name);
    let mut command = Command::new(cargo);
    command
        .args(["build", "--manifest-path"])
        .arg(manifest_dir.join("../notifier/Cargo.toml"))
        .args(["--bin", "p2premote-notifier"]);
    if profile.eq_ignore_ascii_case("release") {
        command.arg("--release");
    }
    command.arg("--target").arg(&target_triple);
    command.env("CARGO_TARGET_DIR", &target_dir);
    command.env("P2PREMOTE_CLIENT_VERSION", client_version());

    let output = command
        .output()
        .unwrap_or_else(|err| panic!("failed to invoke cargo build for {}: {}", binary_name, err));
    if !output.status.success() {
        panic!(
            "failed to build {}: {}",
            binary_name,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    copy_built_file(
        &source,
        &resources_dir.join(&binary_name),
        &resources_dir,
        &binary_name,
    );
}

fn copy_desktop_engine_into_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("resources");
    let (target_os, target_arch) = parse_target();
    if target_os != "windows" {
        return;
    }
    if target_arch != "amd64" {
        panic!("RustDeskTiny integration currently supports Windows amd64 only");
    }
    let installer = resources_dir.join("RustDeskTiny-install.exe");
    if !installer.is_file() {
        panic!("RustDeskTiny installer resource is missing: {}", installer.display());
    }
}

fn copy_directory(source: &Path, target: &Path) {
    fs::create_dir_all(target)
        .unwrap_or_else(|err| panic!("failed to create {}: {}", target.display(), err));
    for entry in fs::read_dir(source)
        .unwrap_or_else(|err| panic!("failed to read {}: {}", source.display(), err))
    {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read directory entry: {err}"));
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &target_path);
        } else {
            println!("cargo:rerun-if-changed={}", source_path.display());
            fs::copy(&source_path, &target_path).unwrap_or_else(|err| {
                panic!(
                    "failed to copy {} to {}: {}",
                    source_path.display(),
                    target_path.display(),
                    err
                )
            });
        }
    }
}

fn ensure_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path)
            .unwrap_or_else(|err| panic!("failed to stat {}: {}", path.display(), err));
        let mode = metadata.permissions().mode();
        fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o755))
            .unwrap_or_else(|err| panic!("failed to make {} executable: {}", path.display(), err));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn copy_built_file(source: &Path, target: &Path, resources_dir: &Path, display_name: &str) {
    if !source.exists() {
        panic!("{} not found at {}", display_name, source.display());
    }

    fs::create_dir_all(resources_dir).unwrap_or_else(|err| {
        panic!(
            "failed to create resources dir {}: {}",
            resources_dir.display(),
            err
        )
    });

    fs::copy(source, target).unwrap_or_else(|err| {
        panic!(
            "failed to copy {} from {} to {}: {}",
            display_name,
            source.display(),
            target.display(),
            err
        )
    });
}

fn resolve_built_service_path(
    target_dir: &Path,
    target_triple: &str,
    profile: &str,
    binary_name: &str,
) -> PathBuf {
    let mut base = target_dir.to_path_buf();
    base.push(target_triple);
    base.push(profile);
    base.push(binary_name);
    base
}
