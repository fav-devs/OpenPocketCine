//! Mirrors the `opc_core_linked` flag from `opc-core-sys`.
//!
//! The window, the camera link and the colour cube all reach the Swift core, so they
//! compile only when there is a core to link against. Without one the binary still
//! builds and says what is missing — which is what lets `cargo test --workspace` run on
//! a machine with no Swift toolchain, since cargo links every binary in the workspace
//! before it runs a single integration test.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(opc_core_linked)");
    println!("cargo:rerun-if-env-changed=DEP_OPENPOCKETCINEDESKTOP_LIB_DIR");
    if std::env::var("DEP_OPENPOCKETCINEDESKTOP_LIB_DIR").is_ok() {
        println!("cargo:rustc-cfg=opc_core_linked");
    }
}
