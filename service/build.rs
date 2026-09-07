use std::env;
use std::path::PathBuf;

fn main() {
    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
        }
        Ok("linux") => {
            emit_punch_link_args();
        }
        _ => {}
    }
}

// The punch .a (Rust rewrite) is std-external: scripts/strip-rustlib.sh in
// p2premote-punch-rs removed its bundled rustlib members so it cannot
// collide with this binary's own std. It must be linked whole: archive pull
// order would otherwise leave the punch members' own references unresolved.
// core/build.rs cannot emit this — cargo would fold the archive into core's
// rlib instead of passing it to the final link.
fn emit_punch_link_args() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"));
    let resources_dir = env::var_os("P2PREMOTE_PUNCH_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("..").join("src-tauri").join("resources"));
    let punch_library = resources_dir.join("libp2premote-punch.a");
    println!("cargo:rerun-if-changed={}", punch_library.display());
    println!("cargo:rustc-link-arg=-Wl,--whole-archive");
    println!("cargo:rustc-link-arg={}", punch_library.display());
    println!("cargo:rustc-link-arg=-Wl,-no-whole-archive");
}
