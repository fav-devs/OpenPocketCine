//! The viewfinder.
//!
//! `opc-monitor view` opens a window on the camera: the picture fills it, a thin strip
//! of chrome top and bottom says what the body is set to, and the keyboard drives the
//! gimbal, the zoom, recording and tracking.
//!
//! Join the camera's Wi-Fi first. Bluetooth pairing will do that from here once the
//! platform BLE transport lands; the state machine behind it is already in `opc-camera`.

use std::process::ExitCode;

/// Show a modal error dialog when there is no terminal to read stderr.
fn show_error(message: &str) {
    #[cfg(target_os = "windows")]
    {
        let title: Vec<u16> = "opc-monitor".encode_utf16().chain(Some(0)).collect();
        let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        #[allow(non_snake_case)]
        extern "system" {
            fn MessageBoxW(hWnd: *mut u8, text: *const u16, caption: *const u16, ty: u32) -> i32;
        }
        // MB_OK | MB_ICONERROR
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x10) };
    }
    #[cfg(not(target_os = "windows"))]
    let _ = message;
}

mod demo;

#[cfg(opc_core_linked)]
mod ble_impl;
#[cfg(opc_core_linked)]
mod connect;
#[cfg(opc_core_linked)]
mod link;
#[cfg(opc_core_linked)]
mod media;
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
    opc-monitor demo
    opc-monitor keys
    opc-monitor version

Join the camera's Wi-Fi first; the viewfinder talks to it directly.
`--camera` points the link somewhere other than the camera's usual address, which is
how a capture or a fake camera is driven.
`demo` opens the UI with a synthetic frame and no camera required.";

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
    // Write a startup log beside the exe so silent crashes leave evidence.
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("opc-monitor.log")));
    let mut log = log_path
        .as_deref()
        .and_then(|p| std::fs::File::create(p).ok());
    macro_rules! log {
        ($($t:tt)*) => {
            if let Some(ref mut f) = log {
                use std::io::Write;
                let _ = writeln!(f, $($t)*);
            }
        };
    }
    log!("opc-monitor starting");

    let args: Vec<String> = std::env::args().skip(1).collect();
    // Catch panics and write them to the log before the process aborts.
    {
        let log_path2 = log_path.clone();
        std::panic::set_hook(Box::new(move |info| {
            let msg = info.to_string();
            if let Some(ref p) = log_path2 {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(p) {
                    let _ = writeln!(f, "PANIC: {msg}");
                }
            }
            eprintln!("opc-monitor panic: {msg}");
        }));
    }

    log!("args: {:?}", args);
    let result = match args.first().map(String::as_str) {
        Some("demo") => {
            log!("opening demo viewfinder");
            demo::run()
        }
        Some("view") => {
            log!("opening viewfinder");
            view_camera(&args[1..])
        }
        None => {
            log!("opening viewfinder");
            view_camera(&[])
        }
        Some("keys") => {
            println!("{KEYS}");
            Ok(())
        }
        Some("version") | Some("--version") => {
            println!("opc-monitor {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => {
            log!("exited ok");
            ExitCode::SUCCESS
        }
        Err(message) => {
            log!("error: {message}");
            eprintln!("opc-monitor: {message}");
            show_error(&message);
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
    use connect::ConnectOutcome;
    use std::path::PathBuf;

    let lut = load_lut(args)?;
    let still = flag(args, "still")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("opc-still.png"));

    // If the caller gave --camera, skip the connection screen and go straight to the feed.
    let remote_str = flag(args, "camera");
    let model_id_str = flag(args, "model");

    let (remote, model_id) = if let Some(text) = remote_str {
        let addr = text
            .parse()
            .map_err(|_| format!("`{text}` is not a host:port"))?;
        let mid = match model_id_str {
            Some(t) => Some(t.parse().map_err(|_| format!("`{t}` is not a model id"))?),
            None => None,
        };
        (Some(addr), mid)
    } else {
        // eframe's pairing window consumes this process's sole winit event loop.
        // Start the actual viewfinder in a fresh process after setup closes.
        let paired_model = match connect::run() {
            ConnectOutcome::Quit => return Ok(()),
            ConnectOutcome::Skip => None,
            ConnectOutcome::Connected {
                ssid,
                password,
                model_id,
            } => {
                join_camera_wifi(&ssid, &password)?;
                model_id
            }
        };
        let requested_model = model_id_str
            .map(|text| {
                text.parse()
                    .map_err(|_| format!("`{text}` is not a model id"))
            })
            .transpose()?;
        relaunch_viewfinder(args, paired_model.or(requested_model))?;
        return Ok(());
    };

    view::run(view::Options {
        remote,
        lut,
        model_id,
        still,
    })
}

#[cfg(opc_core_linked)]
fn relaunch_viewfinder(args: &[String], model_id: Option<i32>) -> Result<(), String> {
    use std::process::Command;

    let mut child_args = vec![
        "view".to_string(),
        "--camera".to_string(),
        "192.168.2.1:9004".to_string(),
    ];
    if let Some(model_id) = model_id {
        child_args.push("--model".to_string());
        child_args.push(model_id.to_string());
    }
    for name in ["look", "lut", "still"] {
        if let Some(value) = flag(args, name) {
            child_args.push(format!("--{name}"));
            child_args.push(value.to_string());
        }
    }
    Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
        .args(&child_args)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not launch viewfinder: {error}"))
}

/// Join the camera SoftAP before the UDP child starts. The Windows profile is manual,
/// so it remains available for the next shoot but Windows will not roam to it on its
/// own. The password exists only in a short-lived profile document and is never logged.
#[cfg(all(opc_core_linked, target_os = "windows"))]
fn join_camera_wifi(ssid: &str, password: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn log(message: &str) {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("opc-monitor.log"))
                {
                    let _ = writeln!(file, "wifi: {message}");
                }
            }
        }
    }

    let profile =
        std::env::temp_dir().join(format!("openpocketcine-wifi-{}.xml", std::process::id()));
    std::fs::write(&profile, opc_camera::wifi::profile_xml(ssid, password))
        .map_err(|error| format!("could not prepare the camera Wi-Fi profile: {error}"))?;
    let add = Command::new("netsh")
        .args(["wlan", "add", "profile"])
        .arg(format!("filename={}", profile.display()))
        .arg("user=current")
        .status();
    let _ = std::fs::remove_file(&profile);
    match add {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return Err(format!(
                "Windows could not save the camera Wi-Fi profile (netsh exit {status})"
            ))
        }
        Err(error) => return Err(format!("could not start Windows Wi-Fi service: {error}")),
    }

    log("requesting connection to the camera network");
    let deadline = Duration::from_secs_f64(opc_camera::wifi::JoinTiming::from_core().deadline);
    let retry_pause =
        Duration::from_secs_f64(opc_camera::wifi::JoinTiming::from_core().retry_pause);
    let started = Instant::now();
    let mut next_attempt = Instant::now();
    while started.elapsed() < deadline {
        if Instant::now() >= next_attempt {
            match Command::new("netsh")
                .args(["wlan", "connect"])
                .arg(format!("name={ssid}"))
                .arg(format!("ssid={ssid}"))
                .status()
            {
                Ok(status) if status.success() => {
                    log("Windows accepted the camera Wi-Fi connection request")
                }
                Ok(status) => log(&format!("Windows Wi-Fi connection request exited {status}")),
                Err(error) => log(&format!(
                    "could not request Windows Wi-Fi connection: {error}"
                )),
            }
            next_attempt = Instant::now() + retry_pause;
        }

        // DHCP on the Osmo network assigns 192.168.2.2 through .254. `ipconfig` is
        // available on every supported Windows host and avoids another platform crate.
        if let Ok(output) = Command::new("ipconfig").output() {
            let addresses = String::from_utf8_lossy(&output.stdout);
            if addresses.split_whitespace().any(|word| {
                word.trim_matches(|c: char| !c.is_ascii_digit() && c != '.')
                    .starts_with("192.168.2.")
            }) {
                log("camera subnet is ready");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    log("timed out waiting for a camera DHCP address");
    Err("Windows did not receive an address from the camera Wi-Fi within 90 seconds.".into())
}

#[cfg(all(opc_core_linked, not(target_os = "windows")))]
fn join_camera_wifi(_ssid: &str, _password: &str) -> Result<(), String> {
    Err("automatic camera Wi-Fi joining is currently implemented for Windows only.".into())
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
