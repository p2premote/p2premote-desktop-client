# Local compatibility patch

Source: crates.io `attohttpc` 0.30.1, licensed under MPL-2.0 (see LICENSE).

`src/request/settings.rs`: add an explicit lifetime to `BaseSettings::headers_mut`
so its `&mut Arc<Self>` receiver compiles with the project's Rust 1.77.2
toolchain. The returned reference still borrows the receiver for the same lifetime;
runtime behavior and the public API are unchanged.
