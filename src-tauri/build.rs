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

    build_punch_library_into_resources();
    copy_service_into_resources();
    copy_cli_into_resources();
    copy_notifier_into_resources();
    tauri_build::build()
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
        "p2premote-punch.dll".to_string()
    } else if target_os == "macos" {
        "libp2premote-punch.dylib".to_string()
    } else if target_os == "linux" {
        "libp2premote-punch.a".to_string()
    } else {
        "libp2premote-punch.so".to_string()
    }
}

fn parse_target() -> (String, String) {
    let target = env::var("TARGET").unwrap_or_default();
    let target_os = if target.contains("windows") {
        "windows"
    } else if target.contains("linux") {
        "linux"
    } else if target.contains("darwin") || target.contains("apple") {
        "macos"
    } else {
        env::consts::OS
    };

    let target_arch = if target.contains("aarch64") {
        "arm64"
    } else if target.contains("x86_64") {
        "amd64"
    } else {
        env::consts::ARCH
    };

    (target_os.to_string(), target_arch.to_string())
}

fn client_version() -> String {
    env::var("P2PREMOTE_CLIENT_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env::var("CARGO_PKG_VERSION").ok())
        .unwrap_or_else(|| "0.0.0".to_string())
}

fn build_punch_library_into_resources() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let source_dir = env::var_os("P2PREMOTE_PUNCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../p2premote-punch"));
    println!("cargo:rerun-if-changed={}", source_dir.display());
    let resources_dir = manifest_dir.join("resources");
    let (target_os, target_arch) = parse_target();
    let library_name = punch_library_name(&target_os);
    let target = resources_dir.join(&library_name);

    if let Err(err) = fs::create_dir_all(&resources_dir) {
        println!(
            "cargo:warning=failed to create resources dir {}: {}",
            resources_dir.display(),
            err
        );
        return;
    }

    let mut command = Command::new("go");
    command.arg("build");
    if target_os == "linux" {
        command.arg("-buildmode=c-archive");
    } else {
        command.arg("-buildmode=c-shared");
    }
    command
        .arg("-ldflags")
        .arg("-s -w")
        .arg("-o")
        .arg(&target)
        .arg("./punchffi");
    command.current_dir(&source_dir);
    command.env("GOOS", &target_os);
    command.env("GOARCH", &target_arch);

    match command.output() {
        Ok(output) if output.status.success() => {
            if target_os != "linux" {
                ensure_executable(&target);
            }
            remove_generated_c_header(&target);
        }
        Ok(output) => {
            println!(
                "cargo:warning=failed to build {}: {}",
                library_name,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            if target.exists() {
                println!(
                    "cargo:warning=using existing {} at {}",
                    library_name,
                    target.display()
                );
                if target_os != "linux" {
                    ensure_executable(&target);
                }
                remove_generated_c_header(&target);
            }
        }
        Err(err) => {
            println!(
                "cargo:warning=failed to invoke go build for {}: {}",
                library_name, err
            );
            if target.exists() {
                println!(
                    "cargo:warning=using existing {} at {}",
                    library_name,
                    target.display()
                );
                if target_os != "linux" {
                    ensure_executable(&target);
                }
                remove_generated_c_header(&target);
            }
        }
    }
}

fn remove_generated_c_header(library_path: &Path) {
    let header_path = library_path.with_extension("h");
    if header_path.exists() {
        let _ = fs::remove_file(header_path);
    }
}

fn copy_service_into_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("resources");
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let service_manifest = manifest_dir.join("../service/Cargo.toml");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let target_triple = env::var("TARGET").ok();
    let target_dir = manifest_dir.join("target").join("service-build");
    let (target_os, _) = parse_target();
    let binary_name = main_binary_name("p2premote-service", &target_os);
    let source = resolve_built_service_path(&target_dir, target_triple.as_deref(), &profile, &binary_name);
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
    if let Some(target) = &target_triple {
        command.arg("--target");
        command.arg(target);
    }
    command.env("CARGO_TARGET_DIR", &target_dir);
    command.env("P2PREMOTE_CLIENT_VERSION", client_version());

    match command.output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            if !source.exists() {
                println!(
                    "cargo:warning=failed to build {}: {}",
                    binary_name,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                return;
            }
        }
        Err(err) => {
            if !source.exists() {
                println!(
                    "cargo:warning=failed to invoke cargo build for {}: {}",
                    binary_name, err
                );
                return;
            }
        }
    }

    copy_built_file(&source, &target, &resources_dir, &binary_name);
    ensure_cross_platform_aliases(&resources_dir, "p2premote-service", &binary_name);
}

fn copy_cli_into_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("resources");
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let cli_manifest = manifest_dir.join("../cli/Cargo.toml");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let target_triple = env::var("TARGET").ok();
    let target_dir = manifest_dir.join("target").join("cli-build");
    let (target_os, _) = parse_target();
    let binary_name = main_binary_name("p2premote-cli", &target_os);
    let source = resolve_built_service_path(
        &target_dir,
        target_triple.as_deref(),
        &profile,
        &binary_name,
    );
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
    if let Some(target) = &target_triple {
        command.arg("--target");
        command.arg(target);
    }
    command.env("CARGO_TARGET_DIR", &target_dir);
    command.env("P2PREMOTE_CLIENT_VERSION", client_version());

    match command.output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            if !source.exists() {
                println!(
                    "cargo:warning=failed to build {}: {}",
                    binary_name,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                return;
            }
        }
        Err(err) => {
            if !source.exists() {
                println!(
                    "cargo:warning=failed to invoke cargo build for {}: {}",
                    binary_name, err
                );
                return;
            }
        }
    }

    copy_built_file(&source, &target, &resources_dir, &binary_name);
    ensure_cross_platform_aliases(&resources_dir, "p2premote-cli", &binary_name);
}

fn copy_notifier_into_resources() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let resources_dir = manifest_dir.join("resources");
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let target_triple = env::var("TARGET").ok();
    let target_dir = manifest_dir.join("target").join("notifier-build");
    let (target_os, _) = parse_target();
    let binary_name = main_binary_name("p2premote-notifier", &target_os);
    let source = resolve_built_service_path(
        &target_dir,
        target_triple.as_deref(),
        &profile,
        &binary_name,
    );
    let mut command = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()));
    command.args(["build", "--manifest-path"])
        .arg(manifest_dir.join("../notifier/Cargo.toml"))
        .args(["--bin", "p2premote-notifier"]);
    if profile.eq_ignore_ascii_case("release") { command.arg("--release"); }
    if let Some(target) = &target_triple { command.arg("--target").arg(target); }
    command.env("CARGO_TARGET_DIR", &target_dir);
    match command.output() {
        Ok(output) if output.status.success() => {}
        Ok(output) if !source.exists() => {
            println!("cargo:warning=failed to build {}: {}", binary_name, String::from_utf8_lossy(&output.stderr).trim());
            return;
        }
        Err(err) if !source.exists() => {
            println!("cargo:warning=failed to invoke cargo build for {}: {}", binary_name, err);
            return;
        }
        _ => {}
    }
    copy_built_file(&source, &resources_dir.join(&binary_name), &resources_dir, &binary_name);
}

fn ensure_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let mode = metadata.permissions().mode();
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o755));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn copy_built_file(source: &Path, target: &Path, resources_dir: &Path, display_name: &str) {
    if !source.exists() {
        println!(
            "cargo:warning={} not found at {}",
            display_name,
            source.display()
        );
        return;
    }

    if let Err(err) = fs::create_dir_all(resources_dir) {
        println!(
            "cargo:warning=failed to create resources dir {}: {}",
            resources_dir.display(),
            err
        );
        return;
    }

    match fs::copy(source, target) {
        Ok(_) => {}
        Err(err) => println!(
            "cargo:warning=failed to copy {} from {} to {}: {}",
            display_name,
            source.display(),
            target.display(),
            err
        ),
    }
}

fn resolve_built_service_path(
    target_dir: &Path,
    target_triple: Option<&str>,
    profile: &str,
    binary_name: &str,
) -> PathBuf {
    let mut base = target_dir.to_path_buf();
    if let Some(target) = target_triple {
        base.push(target);
    }
    base.push(profile);
    base.push(binary_name);
    base
}

fn ensure_cross_platform_aliases(resources_dir: &Path, stem: &str, actual_binary_name: &str) {
    let actual = resources_dir.join(actual_binary_name);
    if !actual.exists() {
        return;
    }

    let aliases = [format!("{}.exe", stem), stem.to_string()];
    for alias in aliases {
        let alias_path = resources_dir.join(alias);
        if alias_path == actual {
            continue;
        }
        let _ = fs::copy(&actual, &alias_path);
    }
}
