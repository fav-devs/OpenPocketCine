//! A window on a shared feed.
//!
//! The relay session runs on its own thread and posts access units; the event loop
//! decodes and presents them. Decoding is not optional per frame — a predicted picture
//! needs the ones before it — so a backlog is skipped forward to a keyframe rather than
//! thinned, which is the only place a frame may be dropped.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use opc_decode::{Codec, Decoder, OwnedPicture};
use opc_relay::ffi::{ControlToken, FrameMeta, Hello, ProtocolInfo, State};
use opc_relay::session::{JoinTarget, Status, WatcherObserver, WatcherOptions, WatcherSession};
use opc_render::{
    write_png, FeedRenderer, GradeOptions, Lut, Peaking, PeakingSense, Presented, Zebra,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// A backlog longer than this means the window stalled; skip to the next keyframe.
const BACKLOG_LIMIT: usize = 30;

/// One access unit off the relay, with the host's own keyframe flag.
struct Unit {
    keyframe: bool,
    bytes: Vec<u8>,
}

/// What the title bar shows, written by the session thread.
#[derive(Default)]
struct Shared {
    status: String,
    title: String,
    camera: String,
    recording: bool,
}

struct Posting {
    units: Sender<Unit>,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<Mutex<bool>>,
}

impl WatcherObserver for Posting {
    fn status_changed(&mut self, status: &Status) {
        let text = match status {
            Status::Connecting => "Connecting…".to_string(),
            Status::Reconnecting(attempt) => format!("Reconnecting ({attempt})…"),
            Status::NeedsPasscode => "Passcode needed".to_string(),
            Status::Live => "Live".to_string(),
            Status::Failed(message) => format!("Failed: {message}"),
        };
        if let Ok(mut shared) = self.shared.lock() {
            shared.status = text;
        }
    }

    fn accepted(&mut self, hello: &Hello) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.title = hello.title();
        }
    }

    fn state_changed(&mut self, state: &State) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.camera = format!(
                "{} {} ISO {} {}",
                state.camera_name, state.format, state.iso, state.shutter
            );
            shared.recording = state.is_recording;
        }
    }

    fn token_changed(&mut self, _token: &ControlToken) {}

    fn picture(&mut self, meta: &FrameMeta, _sets: &[Vec<u8>], access_unit: &[u8]) {
        let _ = self.units.send(Unit {
            keyframe: meta.is_keyframe,
            bytes: access_unit.to_vec(),
        });
    }

    fn should_continue(&mut self) -> bool {
        !*self.stop.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// Assists the operator can toggle from the keyboard.
#[derive(Debug, Clone, Copy, Default)]
struct Toggles {
    zebra: bool,
    peaking: bool,
    mirror: bool,
    graded: bool,
}

impl Toggles {
    fn options(self) -> GradeOptions {
        GradeOptions {
            mirror: self.mirror,
            zebra: self.zebra.then(|| Zebra {
                highlight_color: [1.0, 1.0, 1.0, 1.0],
                ..Zebra::highlight(0.94)
            }),
            peaking: self.peaking.then(|| Peaking {
                sense: PeakingSense::default(),
                color: [1.0, 0.0, 0.0, 1.0],
            }),
            ..GradeOptions::default()
        }
    }
}

struct Watch {
    // Declared before `window` so the renderer, which borrows the window's handles,
    // is torn down first.
    renderer: Option<FeedRenderer>,
    window: Option<Window>,
    decoder: Decoder,
    units: Receiver<Unit>,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<Mutex<bool>>,
    lut: Option<Lut>,
    toggles: Toggles,
    latest: Option<OwnedPicture>,
    still: Option<PathBuf>,
    title: String,
}

impl Watch {
    /// Decodes everything waiting, keeping the newest picture.
    fn drain(&mut self) {
        let mut pending: Vec<Unit> = self.units.try_iter().collect();
        if pending.len() > BACKLOG_LIMIT {
            // Skip to the last keyframe rather than dropping predicted pictures, which
            // would decode into garbage.
            if let Some(at) = pending.iter().rposition(|unit| unit.keyframe) {
                pending.drain(..at);
                self.decoder.flush();
            }
        }
        for unit in pending {
            if self.decoder.send(&unit.bytes).is_err() {
                continue;
            }
            while let Ok(Some(picture)) = self.decoder.receive() {
                self.latest = Some(OwnedPicture::copy_from(&picture));
            }
        }
    }

    fn draw(&mut self) {
        self.drain();
        let (Some(renderer), Some(latest)) = (self.renderer.as_mut(), self.latest.as_ref()) else {
            return;
        };
        let picture = latest.picture();
        let (width, height) = (latest.width, latest.height);
        let options = self.toggles.options();

        if let Some(path) = self.still.take() {
            match renderer
                .render(&picture, (width, height), options)
                .map_err(|error| error.to_string())
                .and_then(|image| write_png(&path, &image))
            {
                Ok(()) => println!("Wrote {}", path.display()),
                Err(error) => eprintln!("Could not write the still: {error}"),
            }
        }

        match renderer.present(&picture, options) {
            Ok(Presented::Shown | Presented::Rebuilt) => {}
            Err(error) => eprintln!("present failed: {error}"),
        }
    }

    fn refresh_title(&mut self) {
        let Ok(shared) = self.shared.lock() else {
            return;
        };
        let assists = [
            ("LUT", self.toggles.graded && self.lut.is_some()),
            ("ZEB", self.toggles.zebra),
            ("PEAK", self.toggles.peaking),
            ("MIRROR", self.toggles.mirror),
        ]
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(" ");
        let title = format!(
            "{} — {}{}{}{}",
            if shared.title.is_empty() {
                &self.title
            } else {
                &shared.title
            },
            shared.status,
            if shared.recording { "  ● REC" } else { "" },
            if shared.camera.is_empty() {
                String::new()
            } else {
                format!("  {}", shared.camera)
            },
            if assists.is_empty() {
                String::new()
            } else {
                format!("  [{assists}]")
            },
        );
        if let Some(window) = self.window.as_ref() {
            window.set_title(&title);
        }
    }

    fn key(&mut self, key: &Key) -> bool {
        match key.as_ref() {
            Key::Named(NamedKey::Escape) => return false,
            Key::Character("z") => self.toggles.zebra = !self.toggles.zebra,
            Key::Character("p") => self.toggles.peaking = !self.toggles.peaking,
            Key::Character("m") => self.toggles.mirror = !self.toggles.mirror,
            Key::Character("l") => {
                self.toggles.graded = !self.toggles.graded;
                let cube = self.toggles.graded.then_some(self.lut.as_ref()).flatten();
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Err(error) = renderer.set_lut(cube) {
                        eprintln!("could not set the cube: {error}");
                    }
                }
            }
            Key::Character("s") => self.still = Some(PathBuf::from("opc-still.png")),
            _ => {}
        }
        self.refresh_title();
        true
    }
}

impl ApplicationHandler for Watch {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("could not open a window: {error}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        let handles = window.display_handle().and_then(|display| {
            window
                .window_handle()
                .map(|w| (display.as_raw(), w.as_raw()))
        });
        let Ok((display, raw_window)) = handles else {
            eprintln!("this window gave no handles to draw into");
            event_loop.exit();
            return;
        };
        // Safety: `window` is stored below and dropped after `renderer`, so both handles
        // outlive every use of them.
        let renderer = unsafe {
            FeedRenderer::for_window(display, raw_window, size.width.max(1), size.height.max(1))
        };
        match renderer {
            Ok(mut renderer) => {
                if self.toggles.graded {
                    let _ = renderer.set_lut(self.lut.as_ref());
                }
                println!("drawing on {}", renderer.device_name());
                self.renderer = Some(renderer);
            }
            Err(error) => {
                eprintln!("could not start the feed pipeline: {error}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        self.refresh_title();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width.max(1), size.height.max(1));
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if !self.key(&event.logical_key) {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => {
                self.draw();
                self.refresh_title();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // The feed sets the pace; redrawing continuously keeps latency at one frame.
        event_loop.set_control_flow(ControlFlow::Poll);
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Ok(mut stop) = self.stop.lock() {
            *stop = true;
        }
    }
}

/// Joins `target` and draws it until the window closes.
pub fn run(
    info: ProtocolInfo,
    target: JoinTarget,
    options: WatcherOptions,
    lut: Option<Lut>,
) -> Result<(), String> {
    let (sender, receiver) = channel();
    let shared = Arc::new(Mutex::new(Shared::default()));
    let stop = Arc::new(Mutex::new(false));
    let title = target.name.clone();

    let session_shared = Arc::clone(&shared);
    let session_stop = Arc::clone(&stop);
    let worker = std::thread::spawn(move || {
        let mut session = WatcherSession::new(info, target, options);
        let mut posting = Posting {
            units: sender,
            shared: session_shared,
            stop: session_stop,
        };
        if let Err(error) = session.run(&mut posting) {
            eprintln!("the feed ended: {error}");
        }
    });

    let decoder = Decoder::new(Codec::Hevc).map_err(|error| error.to_string())?;
    let graded = lut.is_some();
    let mut watch = Watch {
        renderer: None,
        window: None,
        decoder,
        units: receiver,
        shared,
        stop: Arc::clone(&stop),
        lut,
        toggles: Toggles {
            graded,
            ..Toggles::default()
        },
        latest: None,
        still: None,
        title,
    };

    let event_loop = EventLoop::new().map_err(|error| format!("no window system: {error}"))?;
    let outcome = event_loop
        .run_app(&mut watch)
        .map_err(|error| format!("the window closed unexpectedly: {error}"));

    if let Ok(mut flag) = stop.lock() {
        *flag = true;
    }
    let _ = worker.join();
    outcome
}
