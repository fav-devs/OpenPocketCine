//! Locates the Swift facade built by `just desktop-core`.
//!
//! Link flags are emitted only when the library is actually present. That keeps
//! `cargo check` and the pure-Rust layout tests working on a machine with no Swift
//! toolchain, while anything that calls into the core fails loudly at link time
//! instead of silently running a stub.

use std::path::{Path, PathBuf};

const LIB_NAME: &str = "OpenPocketCineDesktop";

fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(dir) = std::env::var("OPC_CORE_LIB_DIR") {
        out.push(PathBuf::from(dir));
    }
    // Apps/Desktop/crates/opc-core-sys -> repository root.
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let root = Path::new(&manifest)
            .ancestors()
            .nth(3)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        out.push(root.join(".build/release"));
        out.push(root.join(".build/debug"));
    }
    out
}

fn has_library(dir: &Path) -> bool {
    ["dylib", "so", "dll"]
        .iter()
        .any(|ext| dir.join(format!("lib{LIB_NAME}.{ext}")).exists())
        || dir.join(format!("{LIB_NAME}.dll")).exists()
        || dir.join(format!("{LIB_NAME}.lib")).exists()
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(opc_core_linked)");
    println!("cargo:rerun-if-env-changed=OPC_CORE_LIB_DIR");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    for dir in candidates() {
        println!("cargo:rerun-if-changed={}", dir.display());
        if !has_library(&dir) {
            continue;
        }
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=dylib={LIB_NAME}");
        // Let the built binary find the library beside the build tree. Windows resolves
        // DLLs from the executable directory instead, so the staging step copies it there.
        if target_os == "linux" || target_os == "macos" {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
        println!("cargo:rustc-cfg=opc_core_linked");
        println!("cargo:lib_dir={}", dir.display());
        return;
    }
    println!(
        "cargo:warning=Swift core library lib{LIB_NAME} not found. Run `just desktop-core` \
         (or set OPC_CORE_LIB_DIR). Layout tests still run; anything calling the core will \
         fail to link."
    );
}
