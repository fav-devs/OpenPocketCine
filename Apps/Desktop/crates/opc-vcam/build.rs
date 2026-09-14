//! Mirrors the `opc_core_linked` flag from `opc-core-sys`, as `opc-monitor` does: the
//! macOS camera reaches the Swift facade and compiles only when there is a core to link.
//! Without one the backend says what is missing.

use std::path::{Path, PathBuf};

const LIB_NAME: &str = "OpenPocketCineDesktop";

fn has_library(dir: &Path) -> bool {
    ["dylib", "so", "dll"]
        .iter()
        .any(|ext| dir.join(format!("lib{LIB_NAME}.{ext}")).exists())
        || dir.join(format!("{LIB_NAME}.dll")).exists()
        || dir.join(format!("{LIB_NAME}.lib")).exists()
}

fn find_core() -> bool {
    // Prefer explicit override.
    if let Ok(dir) = std::env::var("OPC_CORE_LIB_DIR") {
        if has_library(Path::new(&dir)) {
            return true;
        }
    }
    // Fall back to Swift's default output directories relative to the repo root.
    // crates/opc-vcam -> crates -> Apps/Desktop -> Apps -> repo root
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let root = Path::new(&manifest)
            .ancestors()
            .nth(4)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        for candidate in [
            root.join(".build/x86_64-unknown-windows-msvc/debug"),
            root.join(".build/x86_64-unknown-windows-msvc/release"),
            root.join(".build/release"),
            root.join(".build/debug"),
        ] {
            if has_library(&candidate) {
                return true;
            }
        }
    }
    false
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(opc_core_linked)");
    println!("cargo:rerun-if-env-changed=OPC_CORE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=DEP_OPENPOCKETCINEDESKTOP_LIB_DIR");
    // DEP_OPENPOCKETCINEDESKTOP_LIB_DIR reaches us only when opc-core-sys is a direct
    // dependency. Check the library path directly so the flag works transitively too.
    if std::env::var("DEP_OPENPOCKETCINEDESKTOP_LIB_DIR").is_ok() || find_core() {
        println!("cargo:rustc-cfg=opc_core_linked");
    }
}
