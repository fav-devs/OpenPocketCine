//! Compiles the libavcodec shim and points the linker at FFmpeg.
//!
//! On Unix `pkg-config` finds FFmpeg. On Windows, point `FFMPEG_DIR` at a prebuilt
//! FFmpeg (the shared "dev" build from gyan.dev or BtbN works) and this reads
//! `include/` and `lib/` from it.

use std::path::PathBuf;

const LIBRARIES: [&str; 3] = ["avcodec", "avutil", "swscale"];

fn from_ffmpeg_dir() -> Option<Vec<PathBuf>> {
    let root = PathBuf::from(std::env::var("FFMPEG_DIR").ok()?);
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("lib").display()
    );
    for library in LIBRARIES {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
    Some(vec![root.join("include")])
}

fn from_pkg_config() -> Vec<PathBuf> {
    let mut includes = Vec::new();
    for library in LIBRARIES {
        match pkg_config::Config::new().probe(&format!("lib{library}")) {
            Ok(found) => includes.extend(found.include_paths),
            Err(error) => panic!(
                "could not find lib{library}. Install FFmpeg development packages, or set \
                 FFMPEG_DIR to a prebuilt FFmpeg. ({error})"
            ),
        }
    }
    includes
}

fn main() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-changed=csrc/opc_decode.c");
    println!("cargo:rerun-if-changed=include/opc_decode.h");

    let includes = from_ffmpeg_dir().unwrap_or_else(from_pkg_config);
    let mut build = cc::Build::new();
    build.file("csrc/opc_decode.c").include("include");
    for include in includes {
        build.include(include);
    }
    build.compile("opc_decode");
}
