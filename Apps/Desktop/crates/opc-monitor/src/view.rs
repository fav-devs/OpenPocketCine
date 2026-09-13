//! The window.
//!
//! Deliberately thin. It turns winit events into `Key`s and pointer positions, hands
//! them to the [`Shell`](opc_monitor::shell::Shell), and carries out whatever intents come
//! back. Nothing here decides anything an operator would notice.

use std::path::PathBuf;
use std::time::Instant;

use opc_camera::Recovery;
use opc_decode::{Codec, Decoder, OwnedPicture};
use opc_render::{write_png, FeedRenderer, Lut, Presented};
use opc_ui::{Key as UiKey, Phase};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::link::{FromCamera, Link};
use opc_monitor::shell::{Intent, Shell, TouchPhase as Finger};

/// A backlog longer than this means the window stalled. Predicted pictures cannot be
/// thinned — they need the ones before them — so the only safe skip is to a keyframe.
const BACKLOG_LIMIT: usize = 30;

/// How the viewfinder was asked to start.
#[derive(Debug, Default)]
pub struct Options {
    /// An explicit camera address, for a fake camera or an unusual network.
    pub remote: Option<std::net::SocketAddr>,
    pub lut: Option<Lut>,
    /// Which body this is, so the status decoder reads its own encodings.
    pub model_id: Option<i32>,
    pub still: PathBuf,
}

struct Unit {
    keyframe: bool,
    bytes: Vec<u8>,
}

struct View {
    // Declared before `window` so the renderer, which borrows the window's handles, is
    // torn down first.
    renderer: Option<FeedRenderer>,
    window: Option<Window>,
    shell: Shell,
    link: Link,
    decoder: Option<Decoder>,
    pending: Vec<Unit>,
    latest: Option<OwnedPicture>,
    lut: Option<Lut>,
    still: PathBuf,
    take_still: bool,
    pointer: (f64, f64),
    pointer_control: bool,
    started: Instant,
}

impl View {
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn carry_out(&mut self, intents: Vec<Intent>, event_loop: &ActiveEventLoop) {
        for intent in intents {
            match intent {
                Intent::Send(command) => self.link.send(command),
                Intent::Still => self.take_still = true,
                Intent::Quit => event_loop.exit(),
            }
        }
        if let Some(wanted) = self.shell.take_lut_change() {
            let cube = wanted.then_some(self.lut.as_ref()).flatten();
            if let Some(renderer) = self.renderer.as_mut() {
                if let Err(error) = renderer.set_lut(cube) {
                    eprintln!("could not set the cube: {error}");
                }
            }
        }
    }

    /// Takes everything the camera thread has posted.
    fn pump_camera(&mut self) {
        for event in self.link.drain() {
            match event {
                FromCamera::Opened => self.shell.set_phase(Phase::Waiting),
                FromCamera::Picture(bytes) => {
                    // The core marks keyframes; the depacketizer hands over whole access
                    // units, so the first NAL type is enough to know one.
                    let keyframe = is_keyframe(&bytes);
                    self.pending.push(Unit { keyframe, bytes });
                }
                FromCamera::Status(status) => {
                    self.shell.set_status(*status);
                    if matches!(self.shell.phase(), Phase::Waiting) && self.latest.is_some() {
                        self.shell.set_phase(Phase::Live);
                    }
                }
                FromCamera::Recovering(recovery) => {
                    self.shell.set_phase(Phase::Recovering);
                    if matches!(recovery, Recovery::RebuildDecoder) {
                        self.rebuild_decoder();
                    }
                }
                FromCamera::Lost(reason) => self.shell.set_phase(Phase::Failed(reason)),
            }
        }
    }

    /// The watchdog says the decoder is wedged. A fresh one is cheap; a wedged one
    /// produces a black window with a live HUD, which reads as a camera fault.
    fn rebuild_decoder(&mut self) {
        let codec = self
            .decoder
            .as_ref()
            .map(Decoder::codec)
            .unwrap_or(Codec::Hevc);
        match Decoder::new(codec) {
            Ok(decoder) => {
                self.decoder = Some(decoder);
                self.link.note_decoder_failed(false);
                // Everything queued was for the old decoder's state.
                self.pending.clear();
                self.latest = None;
            }
            Err(error) => {
                eprintln!("could not rebuild the decoder: {error}");
                self.decoder = None;
                self.link.note_decoder_failed(true);
            }
        }
    }

    /// Decodes everything waiting, keeping the newest picture.
    fn decode(&mut self) {
        if self.decoder.is_none() {
            let Some(unit) = self.pending.first() else {
                return;
            };
            let codec = codec_of(&unit.bytes).unwrap_or(Codec::Hevc);
            match Decoder::new(codec) {
                Ok(decoder) => self.decoder = Some(decoder),
                Err(error) => {
                    eprintln!("could not create {codec:?} decoder: {error}");
                    self.link.note_decoder_failed(true);
                    return;
                }
            }
        }
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };
        if self.pending.len() > BACKLOG_LIMIT {
            if let Some(at) = self.pending.iter().rposition(|unit| unit.keyframe) {
                self.pending.drain(..at);
                decoder.flush();
            }
        }
        for unit in self.pending.drain(..) {
            if decoder.send(&unit.bytes).is_err() {
                continue;
            }
            while let Ok(Some(picture)) = decoder.receive() {
                self.latest = Some(OwnedPicture::copy_from(&picture));
            }
        }
    }

    fn draw(&mut self) {
        self.pump_camera();
        self.decode();
        let now = self.now();
        let intents = self.shell.tick(now);
        for intent in intents {
            if let Intent::Send(command) = intent {
                self.link.send(command);
            }
        }

        let (Some(renderer), Some(latest)) = (self.renderer.as_mut(), self.latest.as_ref()) else {
            // No picture yet. ControlFlow::Wait (set in about_to_wait) keeps the GPU
            // idle; nothing to present until the first frame arrives.
            return;
        };

        self.shell.set_source(latest.width, latest.height);
        if matches!(self.shell.phase(), Phase::Waiting | Phase::Recovering) {
            self.shell.set_phase(Phase::Live);
        }

        match self.shell.chrome(now) {
            Some(chrome) => renderer.set_overlay(chrome),
            None => renderer.clear_overlay(),
        }

        let picture = latest.picture();
        let options = self.shell.grade_options();

        if self.take_still {
            self.take_still = false;
            let size = (latest.width, latest.height);
            match renderer
                .render(&picture, size, options)
                .map_err(|error| error.to_string())
                .and_then(|image| write_png(&self.still, &image))
            {
                Ok(()) => println!("Wrote {}", self.still.display()),
                Err(error) => eprintln!("could not write the still: {error}"),
            }
        }

        match renderer.present(&picture, options) {
            Ok(Presented::Shown) => {
                self.shell.note_presented(now);
                self.link.note_presented();
            }
            Ok(Presented::Rebuilt) => {}
            Err(error) => eprintln!("present failed: {error}"),
        }
    }
}

/// winit's key to the shell's. Everything unmapped is dropped here rather than in the
/// shell, so the shell's table stays the whole story.
fn translate(key: &Key) -> Option<UiKey> {
    match key.as_ref() {
        Key::Named(NamedKey::Space) => Some(UiKey::Space),
        Key::Named(NamedKey::Escape) => Some(UiKey::Escape),
        Key::Named(NamedKey::ArrowLeft) => Some(UiKey::Left),
        Key::Named(NamedKey::ArrowRight) => Some(UiKey::Right),
        Key::Named(NamedKey::ArrowUp) => Some(UiKey::Up),
        Key::Named(NamedKey::ArrowDown) => Some(UiKey::Down),
        Key::Character(text) => text.chars().next().map(UiKey::Char),
        _ => None,
    }
}

/// Whether this access unit can be decoded on its own.
///
/// Only the first coded slice is read: that NAL's type is the picture's type, and the
/// parameter sets ahead of it say nothing about whether it is a refresh. Scanning past
/// it would be reading a whole megabyte a frame to learn nothing new.
fn is_keyframe(access_unit: &[u8]) -> bool {
    let mut index = 0;
    while index + 4 < access_unit.len() {
        let start = if access_unit[index..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if access_unit[index..].starts_with(&[0, 0, 1]) {
            3
        } else {
            index += 1;
            continue;
        };
        let first = access_unit[index + start];
        if codec_of(access_unit) == Some(Codec::H264) {
            return (first & 0x1F) == 5; // AVC IDR
        }
        let nal_type = (first >> 1) & 0x3F;
        if nal_type <= 31 {
            // A coded slice. 16..=23 are BLA through RASL — the types that refresh.
            return (16..=23).contains(&nal_type);
        }
        index += start;
    }
    false
}

/// The camera declares its codec in the first Annex-B NAL. Most Pockets send HEVC,
/// but this Pocket 3's live stream is AVC (`67` SPS / `65` IDR), so the decoder must
/// follow the bytes rather than a model assumption.
fn codec_of(access_unit: &[u8]) -> Option<Codec> {
    let mut index = 0;
    while index + 4 < access_unit.len() {
        let start = if access_unit[index..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if access_unit[index..].starts_with(&[0, 0, 1]) {
            3
        } else {
            index += 1;
            continue;
        };
        let first = *access_unit.get(index + start)?;
        // AVC parameter sets, IDR and ordinary slices have these unambiguous headers.
        if matches!(first & 0x1F, 5 | 7 | 8) {
            return Some(Codec::H264);
        }
        // HEVC VPS/SPS/PPS use types 32, 33 and 34.
        if matches!((first >> 1) & 0x3F, 32..=34) {
            return Some(Codec::Hevc);
        }
        index += start;
    }
    None
}

impl ApplicationHandler for View {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("OpenPocketCine")
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
                .map(|handle| (display.as_raw(), handle.as_raw()))
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
                if self.shell.toggles().grade {
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
        self.shell.set_window(size.width.max(1), size.height.max(1));
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let now = self.now();
        match event {
            WindowEvent::CloseRequested => {
                self.shell.pointer_cancel();
                let intents = self.shell.control_cancel();
                self.carry_out(intents, event_loop);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let (width, height) = (size.width.max(1), size.height.max(1));
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(width, height);
                }
                self.shell.set_window(width, height);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let Some(key) = translate(&event.logical_key) else {
                    return;
                };
                let intents = match event.state {
                    // A held key repeats. The shell ignores a direction already down,
                    // and the rest are one-shot actions winit does not repeat for us.
                    ElementState::Pressed if event.repeat => return,
                    ElementState::Pressed => self.shell.press(key, now),
                    ElementState::Released => self.shell.release(key, now),
                };
                self.carry_out(intents, event_loop);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x, position.y);
                let intents = if self.pointer_control {
                    self.shell.control_moved(position.x, position.y)
                } else {
                    self.shell.slint_pointer_moved(position.x, position.y);
                    self.shell.pointer_moved(position.x, position.y);
                    Vec::new()
                };
                self.carry_out(intents, event_loop);
            }
            WindowEvent::MouseInput {
                button: MouseButton::Left,
                state,
                ..
            } => {
                let (x, y) = self.pointer;
                match state {
                    ElementState::Pressed => match self.shell.control_down(x, y, now) {
                        Some(intents) => {
                            self.pointer_control = true;
                            self.carry_out(intents, event_loop);
                        }
                        None => self.shell.pointer_down(x, y),
                    },
                    ElementState::Released => {
                        let intents = if self.pointer_control {
                            self.pointer_control = false;
                            self.shell.control_up(x, y, now)
                        } else {
                            self.shell.pointer_up(x, y, now)
                        };
                        self.carry_out(intents, event_loop);
                    }
                }
            }
            WindowEvent::Touch(touch) => {
                let finger = match touch.phase {
                    TouchPhase::Started => Finger::Started,
                    TouchPhase::Moved => Finger::Moved,
                    TouchPhase::Ended => Finger::Ended,
                    TouchPhase::Cancelled => Finger::Cancelled,
                };
                let (x, y) = (touch.location.x, touch.location.y);
                let intents = self.shell.touch(touch.id, finger, x, y, now);
                self.carry_out(intents, event_loop);
            }
            WindowEvent::Focused(false) => {
                self.shell.pointer_cancel();
                let intents = self.shell.control_cancel();
                self.carry_out(intents, event_loop);
                self.pointer_control = false;
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.latest.is_some() {
            // Live feed: poll continuously so latency stays at one frame.
            event_loop.set_control_flow(ControlFlow::Poll);
        } else {
            // No picture yet: sleep until the next event to avoid spinning the GPU.
            event_loop.set_control_flow(ControlFlow::Wait);
        }
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

/// Opens the camera link and a window on it, and runs until the window closes.
pub fn run(options: Options) -> Result<(), String> {
    let (session_id, base_seq) = fresh_session();
    let link = Link::open(options.remote, session_id, base_seq, options.model_id);
    let graded = options.lut.is_some();

    let mut view = View {
        renderer: None,
        window: None,
        shell: Shell::new().with_grade(graded),
        link,
        decoder: None,
        pending: Vec::new(),
        latest: None,
        lut: options.lut,
        still: options.still,
        take_still: false,
        pointer: (0.0, 0.0),
        pointer_control: false,
        started: Instant::now(),
    };
    view.shell.set_phase(Phase::Waiting);

    let event_loop = EventLoop::new().map_err(|error| format!("no window system: {error}"))?;
    event_loop
        .run_app(&mut view)
        .map_err(|error| format!("the window closed unexpectedly: {error}"))
}

/// A session id and an 8-aligned base sequence, fresh per connect. A fixed base can
/// wedge the camera, so this must not be a constant.
fn fresh_session() -> (u16, u16) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.subsec_nanos())
        .unwrap_or(1);
    let session_id = (nanos & 0xFFFF) as u16 | 1;
    let base_seq = ((nanos >> 8) & 0xFFF8) as u16;
    (session_id, base_seq)
}
