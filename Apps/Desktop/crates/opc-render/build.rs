//! Compiles the feed shaders to SPIR-V.
//!
//! Three of the four are the Android shell's own files, compiled from where they live
//! rather than copied. The grade an operator sees on a PC is then the same program that
//! runs on a phone, and a change to it cannot land on one platform only.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `(output name, source path relative to the repository root)`.
const SHARED: [(&str, &str); 5] = [
    (
        "fullscreen.vert",
        "Apps/Android/app/src/main/cpp/shaders/fullscreen.vert",
    ),
    (
        "feed.frag",
        "Apps/Android/app/src/main/cpp/shaders/feed.frag",
    ),
    (
        "blit.frag",
        "Apps/Android/app/src/main/cpp/shaders/blit.frag",
    ),
    (
        "peaking_blur.frag",
        "Apps/Android/app/src/main/cpp/shaders/peaking_blur.frag",
    ),
    (
        "peaking_mask.frag",
        "Apps/Android/app/src/main/cpp/shaders/peaking_mask.frag",
    ),
];

/// Desktop-only: software decode hands over planes, not an AHardwareBuffer.
const LOCAL: [&str; 2] = ["ycbcr.frag", "overlay.frag"];

fn compiler() -> (&'static str, Vec<String>) {
    if Command::new("glslc").arg("--version").output().is_ok() {
        return ("glslc", vec!["-O".to_string()]);
    }
    ("glslangValidator", vec!["-V".to_string()])
}

fn compile(tool: &str, flags: &[String], source: &Path, output: &Path) {
    let result = Command::new(tool)
        .args(flags)
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "could not run {tool}: {error}. Install the Vulkan SDK (glslc) or \
                 glslang-tools (glslangValidator)."
            )
        });
    if !result.status.success() {
        panic!(
            "{tool} failed on {}:\n{}\n{}",
            source.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(opc_core_linked)");
    println!("cargo:rerun-if-env-changed=DEP_OPENPOCKETCINEDESKTOP_LIB_DIR");
    if std::env::var("DEP_OPENPOCKETCINEDESKTOP_LIB_DIR").is_ok() {
        println!("cargo:rustc-cfg=opc_core_linked");
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("a manifest dir"));
    // crates/opc-render -> crates -> Apps/Desktop -> Apps -> repository root.
    let root = manifest
        .ancestors()
        .nth(4)
        .expect("the repository root")
        .to_path_buf();
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("an output dir"));
    let (tool, flags) = compiler();

    for (name, relative) in SHARED {
        let source = root.join(relative);
        println!("cargo:rerun-if-changed={}", source.display());
        assert!(
            source.exists(),
            "shared shader missing: {}. The desktop shell compiles the Android shell's \
             shaders so the two grades cannot drift.",
            source.display()
        );
        compile(tool, &flags, &source, &out.join(format!("{name}.spv")));
    }
    for name in LOCAL {
        let source = manifest.join("shaders").join(name);
        println!("cargo:rerun-if-changed={}", source.display());
        compile(tool, &flags, &source, &out.join(format!("{name}.spv")));
    }
}
