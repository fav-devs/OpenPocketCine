//! Mirrors the `opc_core_linked` flag from `opc-core-sys`.
//!
//! Cargo exposes a `links` crate's metadata to its dependents, so the integration tests
//! that actually call the Swift core compile only when the core is there to link.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(opc_core_linked)");
    println!("cargo:rerun-if-env-changed=DEP_OPENPOCKETCINEDESKTOP_LIB_DIR");
    if std::env::var("DEP_OPENPOCKETCINEDESKTOP_LIB_DIR").is_ok() {
        println!("cargo:rustc-cfg=opc_core_linked");
    }
}
