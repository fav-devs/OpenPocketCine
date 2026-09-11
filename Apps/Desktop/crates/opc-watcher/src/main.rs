//! Milestone 1 of the desktop watcher: prove discovery, join, and picture ingest on a
//! PC before any decoder or GPU work.
//!
//! `--dump` writes the received access units straight to disk. The host already emits
//! Annex-B with parameter sets inline on every keyframe, so that file plays in ffplay or
//! VLC as-is — which is how you confirm the transport is healthy without a renderer.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use opc_relay::discovery::Browser;
use opc_relay::ffi::{self, ControlToken, FrameMeta, Hello, ProtocolInfo, State};
use opc_relay::session::{JoinTarget, Status, WatcherObserver, WatcherOptions, WatcherSession};

const USAGE: &str = "\
OpenPocketCine desktop watcher

USAGE:
    opc-watcher list [--seconds N]
    opc-watcher join <host> [--passcode P] [--as NAME] [--dump PATH] [--seconds N]
    opc-watcher version

Join the camera's Wi-Fi first. Hosts are only advertised on that network.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]),
        Some("join") => join(&args[1..]),
        Some("version") => version(),
        Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("opc-watcher: {message}");
            ExitCode::FAILURE
        }
    }
}

/// A parsed command line: bare words plus `--flag value` pairs.
#[derive(Debug, Default)]
struct Args {
    positional: Vec<String>,
    flags: Vec<(String, String)>,
}

impl Args {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut index = 0;
        while index < args.len() {
            let argument = &args[index];
            if let Some(name) = argument.strip_prefix("--") {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("`--{name}` needs a value"))?;
                parsed.flags.push((name.to_string(), value.clone()));
                index += 2;
            } else {
                parsed.positional.push(argument.clone());
                index += 1;
            }
        }
        Ok(parsed)
    }

    fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn seconds(&self, fallback: u64) -> Result<Duration, String> {
        match self.flag("seconds") {
            Some(value) => value
                .parse::<u64>()
                .map(Duration::from_secs)
                .map_err(|_| format!("`--seconds` wants a whole number, got `{value}`")),
            None => Ok(Duration::from_secs(fallback)),
        }
    }
}

fn protocol() -> Result<ProtocolInfo, String> {
    ProtocolInfo::load()
        .map_err(|error| format!("could not read the core's relay contract: {error}"))
}

fn version() -> Result<(), String> {
    let info = protocol()?;
    let core = ffi::core_version().map_err(|error| error.to_string())?;
    println!("{core}");
    println!("relay protocol v{} on {}", info.version, info.service_type);
    Ok(())
}

fn list(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args)?;
    let window = parsed.seconds(3)?;
    let info = protocol()?;
    let mut browser = Browser::start(&info).map_err(|error| error.to_string())?;
    println!(
        "Looking for shared feeds on this Wi-Fi for {}s…",
        window.as_secs()
    );
    let hosts = browser.poll(window);
    if hosts.is_empty() {
        println!(
            "No shared feeds found. Check that this PC is on the camera's Wi-Fi and that \
             Sharing is on in the host's Operator Setup."
        );
        return Ok(());
    }
    for host in hosts {
        let camera = if host.camera.is_empty() {
            String::new()
        } else {
            format!(" · {}", host.camera)
        };
        let address = host
            .addresses
            .first()
            .map(|address| format!("{address}:{}", host.port))
            .unwrap_or_else(|| "no address".to_string());
        println!("  {}{camera}  ({address})", host.name);
    }
    Ok(())
}

fn join(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args)?;
    let wanted = parsed
        .positional
        .first()
        .ok_or_else(|| format!("`join` needs a host name\n\n{USAGE}"))?;
    let window = parsed.seconds(0)?;
    let info = protocol()?;

    let mut browser = Browser::start(&info).map_err(|error| error.to_string())?;
    println!("Looking for `{wanted}`…");
    let mut found = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(host) = browser
            .poll(Duration::from_millis(500))
            .into_iter()
            .find(|host| host.name.eq_ignore_ascii_case(wanted))
        {
            found = Some(host);
            break;
        }
    }
    let host = found.ok_or_else(|| {
        format!("no shared feed named `{wanted}` on this Wi-Fi. Run `opc-watcher list` first.")
    })?;

    let dump = match parsed.flag("dump") {
        Some(path) => Some(
            File::create(path)
                .map(BufWriter::new)
                .map_err(|error| format!("could not open `{path}`: {error}"))?,
        ),
        None => None,
    };

    let options = WatcherOptions {
        device_name: parsed.flag("as").unwrap_or("Desktop watcher").to_string(),
        watcher_id: format!("desktop-{}", std::process::id()),
        passcode: parsed.flag("passcode").unwrap_or_default().to_string(),
        ..WatcherOptions::default()
    };

    let target = JoinTarget {
        name: host.name.clone(),
        addresses: host.addresses.clone(),
        port: host.port,
    };
    let mut session = WatcherSession::new(info, target, options);
    let mut reporter = Reporter::new(dump, window);
    session
        .run(&mut reporter)
        .map_err(|error| format!("the feed ended: {error}"))?;
    reporter.finish();

    if let Status::Failed(message) = session.status() {
        return Err(message.clone());
    }
    if session.status() == &Status::NeedsPasscode {
        return Err("this feed needs a passcode — pass `--passcode`".to_string());
    }
    Ok(())
}

/// Prints what arrives and, when asked, writes the elementary stream to disk.
struct Reporter {
    dump: Option<BufWriter<File>>,
    stop_after: Duration,
    started: Instant,
    frames: u64,
    keyframes: u64,
    bytes: u64,
    window_started: Instant,
    window_frames: u64,
    last_line: String,
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reporter")
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl Reporter {
    fn new(dump: Option<BufWriter<File>>, stop_after: Duration) -> Self {
        let now = Instant::now();
        Self {
            dump,
            stop_after,
            started: now,
            frames: 0,
            keyframes: 0,
            bytes: 0,
            window_started: now,
            window_frames: 0,
            last_line: String::new(),
        }
    }

    fn finish(&mut self) {
        if let Some(dump) = self.dump.as_mut() {
            let _ = dump.flush();
        }
        if self.frames == 0 {
            println!("No picture arrived.");
            return;
        }
        let elapsed = self.started.elapsed().as_secs_f64().max(0.001);
        println!(
            "\n{} frames ({} keyframes), {:.1} MiB, {:.1} fps average.",
            self.frames,
            self.keyframes,
            self.bytes as f64 / (1024.0 * 1024.0),
            self.frames as f64 / elapsed
        );
    }
}

impl WatcherObserver for Reporter {
    fn status_changed(&mut self, status: &Status) {
        match status {
            Status::Connecting => println!("Connecting…"),
            Status::Reconnecting(attempt) => println!("Reconnecting (attempt {attempt})…"),
            Status::NeedsPasscode => println!("This feed needs a passcode."),
            Status::Live => println!("Joined."),
            Status::Failed(message) => println!("Failed: {message}"),
        }
    }

    fn accepted(&mut self, hello: &Hello) {
        println!("Watching {}", hello.title());
    }

    fn state_changed(&mut self, state: &State) {
        let line = format!(
            "{}  {}  {}  ISO {}  {}  {}%  {}",
            state.camera_name,
            state.format,
            state.color,
            state.iso,
            state.shutter,
            state.battery_percent,
            if state.is_recording { "REC" } else { "—" }
        );
        if line != self.last_line {
            println!("{line}");
            self.last_line = line;
        }
    }

    fn token_changed(&mut self, token: &ControlToken) {
        if token.holder_is_recipient {
            println!("Control granted to this watcher.");
        } else {
            println!("Control held by {}.", token.holder_name);
        }
    }

    fn picture(&mut self, meta: &FrameMeta, parameter_sets: &[Vec<u8>], access_unit: &[u8]) {
        self.frames += 1;
        self.window_frames += 1;
        self.bytes += access_unit.len() as u64;
        if meta.is_keyframe {
            self.keyframes += 1;
            if self.keyframes == 1 {
                println!(
                    "First keyframe: {} bytes, {} parameter set(s).",
                    access_unit.len(),
                    parameter_sets.len()
                );
            }
        }
        if let Some(dump) = self.dump.as_mut() {
            // Access units already carry Annex-B start codes, and a keyframe carries its
            // parameter sets inline, so this is a playable elementary stream.
            let _ = dump.write_all(access_unit);
        }
        let elapsed = self.window_started.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let fps = self.window_frames as f64 / elapsed.as_secs_f64();
            print!("\r{fps:.0} fps  {} frames  ", self.frames);
            let _ = std::io::stdout().flush();
            self.window_started = Instant::now();
            self.window_frames = 0;
        }
    }

    fn should_continue(&mut self) -> bool {
        self.stop_after.is_zero() || self.started.elapsed() < self.stop_after
    }
}
