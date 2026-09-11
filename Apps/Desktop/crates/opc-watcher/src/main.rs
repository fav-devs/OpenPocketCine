//! Milestone 1 of the desktop watcher: prove discovery, join, and picture ingest on a
//! PC before any decoder or GPU work.
//!
//! `--dump` writes the received access units straight to disk. The host already emits
//! Annex-B with parameter sets inline on every keyframe, so that file plays in ffplay or
//! VLC as-is — which is how you confirm the transport is healthy without a renderer.

mod watch;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use opc_decode::{annexb, Codec, Decoder};
use opc_relay::discovery::Browser;
use opc_relay::ffi::{self, ControlToken, FrameMeta, Hello, ProtocolInfo, State};
use opc_relay::session::{JoinTarget, Status, WatcherObserver, WatcherOptions, WatcherSession};
use opc_render::{built_in_names, write_png, FeedRenderer, GradeOptions, Lut};

const USAGE: &str = "\
OpenPocketCine desktop watcher

USAGE:
    opc-watcher list [--seconds N]
    opc-watcher join <host> [--passcode P] [--as NAME] [--dump PATH] [--seconds N]
                     [--still PATH] [--look NAME | --lut FILE]
    opc-watcher watch <host> [--passcode P] [--as NAME] [--look NAME | --lut FILE]
    opc-watcher decode <file.h265> [--out DIR] [--frames N] [--display WxH]
                       [--lut FILE | --look NAME] [--mirror] [--upscale]
    opc-watcher version

Join the camera's Wi-Fi first. Hosts are only advertised on that network.
`watch` opens a window on a shared feed. Keys: L cube, Z zebra, P peaking, M mirror,
S still, Esc quit.
`--still` decodes the live feed and writes the first picture that arrives as a PNG,
which is how the whole path gets confirmed before there is a window to draw in.
`decode` replays a `--dump` file through the same decoder and feed pipeline — the way
to check a capture without a camera in the room.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]),
        Some("join") => join(&args[1..]),
        Some("decode") => decode(&args[1..]),
        Some("watch") => watch_feed(&args[1..]),
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
    match FeedRenderer::new() {
        Ok(renderer) => println!("feed pipeline on {}", renderer.device_name()),
        Err(error) => println!("feed pipeline unavailable: {error}"),
    }
    println!("built-in looks: {}", built_in_names().join(", "));
    Ok(())
}

/// Parses `WxH`.
fn raster(value: &str) -> Result<(u32, u32), String> {
    let (width, height) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("`--display` wants WxH, got `{value}`"))?;
    let parsed = |text: &str| {
        text.trim()
            .parse::<u32>()
            .map_err(|_| format!("`--display` wants WxH, got `{value}`"))
    };
    Ok((parsed(width)?, parsed(height)?))
}

fn chosen_lut(parsed: &Args) -> Result<Option<Lut>, String> {
    if let Some(path) = parsed.flag("lut") {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("could not read `{path}`: {error}"))?;
        return Lut::parse(&text)
            .map(Some)
            .map_err(|error| error.to_string());
    }
    if let Some(name) = parsed.flag("look") {
        return Lut::built_in(name, 33)
            .map(Some)
            .map_err(|error| format!("{error} Known looks: {}", built_in_names().join(", ")));
    }
    Ok(None)
}

fn decode(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args)?;
    let source = parsed
        .positional
        .first()
        .ok_or_else(|| format!("`decode` needs a file\n\n{USAGE}"))?;
    let stream =
        std::fs::read(source).map_err(|error| format!("could not read `{source}`: {error}"))?;
    let units = annexb::access_units(&stream);
    if units.is_empty() {
        return Err(format!("`{source}` holds no access units"));
    }
    println!("{} access units in {source}", units.len());

    let limit = match parsed.flag("frames") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| format!("`--frames` wants a whole number, got `{value}`"))?,
        None => usize::MAX,
    };
    let out = parsed.flag("out").map(PathBuf::from);
    if let Some(directory) = out.as_ref() {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("could not make `{}`: {error}", directory.display()))?;
    }

    let mut decoder = Decoder::new(Codec::Hevc).map_err(|error| error.to_string())?;
    let mut pipeline = match out.as_ref() {
        Some(_) => Some(
            FeedRenderer::new()
                .map_err(|error| format!("{error}. Writing stills needs a Vulkan driver."))?,
        ),
        None => None,
    };
    if let Some(renderer) = pipeline.as_mut() {
        println!("drawing on {}", renderer.device_name());
        renderer
            .set_lut(chosen_lut(&parsed)?.as_ref())
            .map_err(|error| error.to_string())?;
    }

    let options = GradeOptions {
        mirror: parsed.flag("mirror").is_some_and(|value| value != "false"),
        upscale: parsed.flag("upscale").is_some_and(|value| value != "false"),
        ..GradeOptions::default()
    };
    let display = match parsed.flag("display") {
        Some(value) => Some(raster(value)?),
        None => None,
    };

    let started = Instant::now();
    let mut decoded = 0usize;
    let mut keyframes = 0usize;
    for unit in units {
        if decoded >= limit {
            break;
        }
        decoder.send(unit).map_err(|error| error.to_string())?;
        while let Some(picture) = decoder.receive().map_err(|error| error.to_string())? {
            if picture.is_keyframe {
                keyframes += 1;
            }
            decoded += 1;
            if let (Some(renderer), Some(directory)) = (pipeline.as_mut(), out.as_ref()) {
                let raster = display.unwrap_or((picture.width, picture.height));
                let image = renderer
                    .render(&picture, raster, options)
                    .map_err(|error| error.to_string())?;
                let path = directory.join(format!("frame-{decoded:05}.png"));
                write_png(&path, &image)?;
            }
            if decoded >= limit {
                break;
            }
        }
    }

    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    println!(
        "{decoded} pictures ({keyframes} keyframes) in {elapsed:.2}s — {:.1} fps",
        decoded as f64 / elapsed
    );
    if let Some(directory) = out.as_ref() {
        println!("stills in {}", directory.display());
    }
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

/// Browses until a host with this name answers.
fn find_host(info: &ProtocolInfo, wanted: &str) -> Result<JoinTarget, String> {
    let mut browser = Browser::start(info).map_err(|error| error.to_string())?;
    println!("Looking for `{wanted}`…");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(host) = browser
            .poll(Duration::from_millis(500))
            .into_iter()
            .find(|host| host.name.eq_ignore_ascii_case(wanted))
        {
            return Ok(JoinTarget {
                name: host.name,
                addresses: host.addresses,
                port: host.port,
            });
        }
    }
    Err(format!(
        "no shared feed named `{wanted}` on this Wi-Fi. Run `opc-watcher list` first."
    ))
}

fn watcher_options(parsed: &Args) -> WatcherOptions {
    WatcherOptions {
        device_name: parsed.flag("as").unwrap_or("Desktop watcher").to_string(),
        watcher_id: format!("desktop-{}", std::process::id()),
        passcode: parsed.flag("passcode").unwrap_or_default().to_string(),
        ..WatcherOptions::default()
    }
}

fn watch_feed(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args)?;
    let wanted = parsed
        .positional
        .first()
        .ok_or_else(|| format!("`watch` needs a host name\n\n{USAGE}"))?;
    let info = protocol()?;
    let target = find_host(&info, wanted)?;
    let lut = chosen_lut(&parsed)?;
    watch::run(info, target, watcher_options(&parsed), lut)
}

fn join(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args)?;
    let wanted = parsed
        .positional
        .first()
        .ok_or_else(|| format!("`join` needs a host name\n\n{USAGE}"))?;
    let window = parsed.seconds(0)?;
    let info = protocol()?;
    let target = find_host(&info, wanted)?;

    let dump = match parsed.flag("dump") {
        Some(path) => Some(
            File::create(path)
                .map(BufWriter::new)
                .map_err(|error| format!("could not open `{path}`: {error}"))?,
        ),
        None => None,
    };

    let still = match parsed.flag("still") {
        Some(path) => Some(LiveStill::new(PathBuf::from(path), chosen_lut(&parsed)?)?),
        None => None,
    };

    let options = watcher_options(&parsed);
    let mut session = WatcherSession::new(info, target, options);
    let mut reporter = Reporter::new(dump, window, still);
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

/// Decodes the live feed until one picture has been written out.
///
/// One still, not a stream of them: the point is to confirm the path end to end, and a
/// watcher that wrote a PNG per frame would spend its time on disk rather than on the
/// feed.
struct LiveStill {
    path: PathBuf,
    decoder: Decoder,
    renderer: FeedRenderer,
    done: bool,
}

impl LiveStill {
    fn new(path: PathBuf, lut: Option<Lut>) -> Result<Self, String> {
        let decoder = Decoder::new(Codec::Hevc).map_err(|error| error.to_string())?;
        let mut renderer = FeedRenderer::new()
            .map_err(|error| format!("{error}. Writing a still needs a Vulkan driver."))?;
        renderer
            .set_lut(lut.as_ref())
            .map_err(|error| error.to_string())?;
        Ok(Self {
            path,
            decoder,
            renderer,
            done: false,
        })
    }

    /// Returns a message once a picture has been written.
    fn offer(&mut self, access_unit: &[u8]) -> Option<String> {
        if self.done {
            return None;
        }
        if self.decoder.send(access_unit).is_err() {
            return None;
        }
        while let Ok(Some(picture)) = self.decoder.receive() {
            let raster = (picture.width, picture.height);
            let Ok(image) = self
                .renderer
                .render(&picture, raster, GradeOptions::default())
            else {
                continue;
            };
            self.done = true;
            return Some(match write_png(&self.path, &image) {
                Ok(()) => format!(
                    "Wrote {}x{} still to {}",
                    image.width,
                    image.height,
                    self.path.display()
                ),
                Err(error) => format!("Could not write the still: {error}"),
            });
        }
        None
    }
}

/// Prints what arrives and, when asked, writes the elementary stream to disk.
struct Reporter {
    dump: Option<BufWriter<File>>,
    still: Option<LiveStill>,
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
    fn new(dump: Option<BufWriter<File>>, stop_after: Duration, still: Option<LiveStill>) -> Self {
        let now = Instant::now();
        Self {
            dump,
            still,
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
        if let Some(message) = self
            .still
            .as_mut()
            .and_then(|still| still.offer(access_unit))
        {
            println!("\n{message}");
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
