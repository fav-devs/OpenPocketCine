//! The viewfinder.
//!
//! `opc-monitor view` opens a window on the camera: the picture fills it, a thin strip
//! of chrome top and bottom says what the body is set to, and the keyboard drives the
//! gimbal, the zoom, recording and tracking.
//!
//! Join the camera's Wi-Fi first. Bluetooth pairing will do that from here once the
//! platform BLE transport lands; the state machine behind it is already in `opc-camera`.

use std::process::ExitCode;

#[cfg(opc_core_linked)]
mod link;
#[cfg(opc_core_linked)]
mod view;

#[cfg(not(opc_core_linked))]
const NO_CORE: &str = "this build has no Swift core linked, so it cannot open a camera. \
                       Build it with `just desktop-build`, which stages the core first.";

const USAGE: &str = "\
OpenPocketCine desktop viewfinder

USAGE:
    opc-monitor view [--camera HOST:PORT] [--look NAME | --lut FILE] [--model ID]
                     [--still PATH]
    opc-monitor keys
    opc-monitor version

Join the camera's Wi-Fi first; the viewfinder talks to it directly.
`--camera` points the link somewhere other than the camera's usual address, which is
how a capture or a fake camera is driven.";

const KEYS: &str = "\
Viewfinder keys

  Space        start recording            T      3-second countdown, or cancel it
  R            stop recording             S      write a still
  Arrows       pan and tilt               C      recentre the gimbal
  + / -        zoom in and out            F      flip to selfie and back
  0            back to wide               Esc    close

  Drag         track what you drew around X      stop tracking
               (mouse or one finger)
  [ / ]        step resolution / frame rate

  Z zebra      P peaking      L colour cube      M mirror      H hide the chrome

Two arrows at once pan diagonally. The gimbal keeps moving while a key is held and
rests the moment it comes up.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("view") => view_camera(&args[1..]),
        Some("keys") => {
            println!("{KEYS}");
            Ok(())
        }
        Some("version") => {
            println!("opc-monitor {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("opc-monitor: {message}");
            ExitCode::FAILURE
        }
    }
}

/// `--flag value` pairs. Nothing here needs positional arguments.
#[cfg(opc_core_linked)]
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|argument| argument == &format!("--{name}"))
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

#[cfg(not(opc_core_linked))]
fn view_camera(_args: &[String]) -> Result<(), String> {
    Err(NO_CORE.to_string())
}

#[cfg(opc_core_linked)]
fn view_camera(args: &[String]) -> Result<(), String> {
    use std::path::PathBuf;

    let remote = match flag(args, "camera") {
        Some(text) => Some(
            text.parse()
                .map_err(|_| format!("`{text}` is not a host:port"))?,
        ),
        None => None,
    };
    let lut = load_lut(args)?;
    let model_id = match flag(args, "model") {
        Some(text) => Some(
            text.parse()
                .map_err(|_| format!("`{text}` is not a model id"))?,
        ),
        None => None,
    };
    let still = flag(args, "still")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("opc-still.png"));

    view::run(view::Options {
        remote,
        lut,
        model_id,
        still,
    })
}

#[cfg(opc_core_linked)]
fn load_lut(args: &[String]) -> Result<Option<opc_render::Lut>, String> {
    use opc_render::{built_in_names, Lut};

    if let Some(path) = flag(args, "lut") {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {path}: {error}"))?;
        return Lut::parse(&text)
            .map(Some)
            .map_err(|error| error.to_string());
    }
    if let Some(name) = flag(args, "look") {
        // 33 is the lattice the shells build the built-in looks at.
        return Lut::built_in(name, 33).map(Some).map_err(|_| {
            format!(
                "no look called `{name}`. Try one of: {}",
                built_in_names().join(", ")
            )
        });
    }
    Ok(None)
}
