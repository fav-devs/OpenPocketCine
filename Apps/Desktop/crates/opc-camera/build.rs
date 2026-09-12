//! Mirrors the `opc_core_linked` flag from `opc-core-sys` so the tests that call the
//! core compile only when there is a core to link.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(opc_core_linked)");
    println!("cargo:rerun-if-env-changed=DEP_OPENPOCKETCINEDESKTOP_LIB_DIR");
    if std::env::var("DEP_OPENPOCKETCINEDESKTOP_LIB_DIR").is_ok() {
        println!("cargo:rustc-cfg=opc_core_linked");
    }
}
