//! The window.
//!
//! Deliberately thin. It turns winit events into `Key`s and pointer positions, hands
//! them to the [`Shell`](opc_monitor::shell::Shell), and carries out whatever intents come
//! back. Nothing here decides anything an operator would notice.

use std::path::PathBuf;
use std::time::Instant;

use opc_camera::Recovery;
use opc_chrome::Screen;
use opc_decode::{Codec, Decoder, OwnedPicture};
use opc_render::{write_png, FeedRenderer, Lut, Presented};
use opc_vcam::{ComponentReport, VirtualCamera};

use crate::media::MediaDriver;
use opc_monitor::luts;
use opc_monitor::{LutChoice, LutRequest, PadButton};
use opc_ui::{Key as UiKey, Phase};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

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
    /// The false-colour paint and weight lattices, while that assist is on.
    false_color: Option<(Lut, Lut)>,
    still: PathBuf,
    take_still: bool,
    pointer: (f64, f64),
    pointer_control: bool,
    started: Instant,
    /// When the plates last read the picture.
    last_scope_at: f64,
    /// A game controller, when the platform offers one.
    pad: Option<gilrs::Gilrs>,
    last_pad_at: f64,
    /// Where the camera is, for a reconnect.
    remote: Option<std::net::SocketAddr>,
    model_id: Option<i32>,
    media: MediaDriver,
    /// A name for the cache folder: the body's model id, or "camera".
    camera_id: String,
    /// The viewfinder as a camera for other apps, while the operator has it on.
    vcam: Option<VirtualCamera>,
    /// When the camera last took a frame, and when its readout was last refreshed.
    last_vcam_at: f64,
    last_vcam_status_at: f64,
    /// A probe, install or remove of the platform camera component, running on its
    /// own thread; the answer lands in the Output tab.
    component_job: Option<std::sync::mpsc::Receiver<ComponentReport>>,
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
                Intent::ToggleFullscreen => {
                    if let Some(window) = self.window.as_ref() {
                        let wanted = window.fullscreen().is_none();
                        window.set_fullscreen(wanted.then_some(Fullscreen::Borderless(None)));
                    }
                }
                Intent::Media(action) => {
                    let now = self.now();
                    let camera_id = self.camera_id.clone();
                    for command in self.media.action(&mut self.shell, action, &camera_id, now) {
                        self.link.send(command);
                    }
                }
                Intent::Reconnect => self.reconnect(),
                Intent::Diagnostics => self.write_diagnostics(),
                Intent::ComponentInstall => self.start_component_job(opc_vcam::install::install),
                Intent::ComponentRemove => self.start_component_job(opc_vcam::install::remove),
                Intent::OpenUrl(url) => self.open_url(&url),
            }
        }
        while let Some(request) = self.shell.take_lut_change() {
            self.apply_lut_request(request);
        }
    }

    /// A cube change from the `L` key or the LUT row: load what was picked, then tell
    /// the renderer. Loading happens here because the built-in looks come from the core.
    fn apply_lut_request(&mut self, request: LutRequest) {
        let on = match request {
            LutRequest::FalseColor(key) => {
                self.false_color = key.and_then(|key| {
                    let paint = Lut::false_color(key.scale, key.color_mode, key.iso, true);
                    let weight = Lut::false_color(key.scale, key.color_mode, key.iso, false);
                    match (paint, weight) {
                        (Ok(paint), Ok(weight)) => Some((paint, weight)),
                        (Err(error), _) | (_, Err(error)) => {
                            eprintln!("could not load false colour: {error}");
                            None
                        }
                    }
                });
                let cubes = self
                    .false_color
                    .as_ref()
                    .map(|(paint, weight)| (paint, weight));
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Err(error) = renderer.set_false_color(cubes) {
                        eprintln!("could not set false colour: {error}");
                    }
                }
                return;
            }
            LutRequest::Toggle(on) => on,
            LutRequest::Load(choice) => {
                let loaded: Result<Option<Lut>, String> = match &choice {
                    LutChoice::Off => Ok(None),
                    // 33 is the lattice the shells build the built-in looks at.
                    LutChoice::BuiltIn(name) => Lut::built_in(name, 33)
                        .map(Some)
                        .map_err(|error| error.to_string()),
                    LutChoice::File(file) => {
                        std::fs::read_to_string(luts::custom_folder().join(file))
                            .map_err(|error| error.to_string())
                            .and_then(|text| Lut::parse(&text).map_err(|error| error.to_string()))
                            .map(Some)
                    }
                };
                match loaded {
                    Ok(cube) => {
                        if cube.is_some() {
                            self.lut = cube;
                        }
                        choice != LutChoice::Off
                    }
                    Err(error) => {
                        eprintln!("could not load the cube: {error}");
                        false
                    }
                }
            }
        };
        let cube = on.then_some(self.lut.as_ref()).flatten();
        if let Some(renderer) = self.renderer.as_mut() {
            if let Err(error) = renderer.set_lut(cube) {
                eprintln!("could not set the cube: {error}");
            }
        }
    }

    /// Tears the datalink down and opens a fresh one, as the Link tab asks.
    fn reconnect(&mut self) {
        let (session_id, base_seq) = fresh_session();
        self.link = Link::open(self.remote, session_id, base_seq, self.model_id);
        self.latest = None;
        self.shell.set_phase(Phase::Waiting);
        self.shell.say("RECONNECTING");
    }

    /// Writes everything a bug report needs next to the LUT folder.
    fn write_diagnostics(&mut self) {
        let now = self.now();
        let mut text = self.shell.diagnostics_text(now);
        text.push_str(&format!(
            "decoder {:?}\n",
            self.decoder.as_ref().map(|d| d.codec())
        ));
        let folder = opc_monitor::prefs::path()
            .parent()
            .map_or_else(|| PathBuf::from("."), |p| p.to_path_buf());
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let path = folder.join(format!("diagnostics-{stamp}.txt"));
        match std::fs::create_dir_all(&folder).and_then(|()| std::fs::write(&path, text)) {
            Ok(()) => {
                println!("Wrote {}", path.display());
                self.shell.say("DIAGNOSTICS WRITTEN TO THE CACHE FOLDER");
            }
            Err(error) => {
                eprintln!("could not write diagnostics: {error}");
                self.shell.say("COULD NOT WRITE DIAGNOSTICS");
            }
        }
    }

    /// Intents raised between frames rather than by a key or a tap: sends and media
    /// fetches. Nothing here can close the window.
    fn carry_out_quietly(&mut self, intents: Vec<Intent>, now: f64) {
        for intent in intents {
            match intent {
                Intent::Send(command) => self.link.send(command),
                Intent::Media(action) => {
                    let camera_id = self.camera_id.clone();
                    for command in self.media.action(&mut self.shell, action, &camera_id, now) {
                        self.link.send(command);
                    }
                }
                Intent::Reconnect => self.reconnect(),
                Intent::Diagnostics => self.write_diagnostics(),
                Intent::ComponentInstall => self.start_component_job(opc_vcam::install::install),
                Intent::ComponentRemove => self.start_component_job(opc_vcam::install::remove),
                Intent::OpenUrl(url) => self.open_url(&url),
                Intent::Still | Intent::Quit | Intent::ToggleFullscreen => {}
            }
        }
    }

    /// Runs a component action off the window thread; one at a time.
    fn start_component_job(&mut self, job: fn() -> ComponentReport) {
        if self.component_job.is_some() {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("opc-component".to_string())
            .spawn(move || {
                let _ = sender.send(job());
            })
            .ok();
        self.component_job = Some(receiver);
    }

    /// Takes a finished component job's answer to the Output tab, and restarts the
    /// camera so it sees what changed.
    fn poll_component_job(&mut self) {
        let Some(receiver) = self.component_job.as_ref() else {
            return;
        };
        let Ok(report) = receiver.try_recv() else {
            return;
        };
        self.component_job = None;
        let installed = report.state == opc_vcam::ComponentState::Installed;
        self.shell.set_component(report);
        if let Some(camera) = self.vcam.take() {
            camera.stop();
        }
        if installed && self.shell.vcam_backend() == Some(opc_vcam::Backend::Device) {
            self.shell.say("CAMERA COMPONENT INSTALLED · CAMERA ON");
        }
    }

    fn open_url(&mut self, url: &str) {
        if let Err(error) = opc_vcam::install::open_url(url) {
            eprintln!("{error}");
            self.shell.say("COULD NOT OPEN THE BROWSER");
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
                FromCamera::Frame(frame) => {
                    let now = self.now();
                    if (frame.cmd_set, frame.cmd_id) == (0x02, 0xA5) {
                        if let Some(poll) = opc_camera::tracking_poll(&frame.payload) {
                            self.shell.tracking_reply(poll, now);
                        }
                    } else {
                        self.media.frame(frame, now);
                    }
                }
                FromCamera::Set(outcome) => self.shell.note_set(outcome),
                FromCamera::Status(status) => {
                    self.media.status(status.in_playback);
                    self.shell.set_status(*status);
                    if matches!(self.shell.phase(), Phase::Waiting) && self.latest.is_some() {
                        self.shell.set_phase(Phase::Live);
                    }
                }
                FromCamera::Recovering(recovery) => {
                    self.shell.set_phase(Phase::Recovering);
                    self.shell.note_recovery(&format!("{recovery:?}"));
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
        self.sample_scopes();
    }

    /// A connected game controller, on the phones' map. Polled once per frame.
    fn poll_pad(&mut self, now: f64) {
        let Some(pad) = self.pad.as_mut() else {
            return;
        };
        let mut intents = Vec::new();
        while let Some(gilrs::Event { id, event, .. }) = pad.next_event() {
            match event {
                gilrs::EventType::Connected => {
                    let name = pad.gamepad(id).name().to_string();
                    self.shell.set_gamepad(Some(name));
                }
                gilrs::EventType::Disconnected => self.shell.set_gamepad(None),
                gilrs::EventType::ButtonPressed(button, _) => {
                    let mapped = match button {
                        gilrs::Button::South => Some(PadButton::A),
                        gilrs::Button::East => Some(PadButton::B),
                        gilrs::Button::West => Some(PadButton::X),
                        gilrs::Button::North => Some(PadButton::Y),
                        gilrs::Button::LeftTrigger => Some(PadButton::LeftShoulder),
                        gilrs::Button::RightTrigger => Some(PadButton::RightShoulder),
                        gilrs::Button::DPadUp => Some(PadButton::DpadUp),
                        gilrs::Button::DPadDown => Some(PadButton::DpadDown),
                        gilrs::Button::DPadLeft => Some(PadButton::DpadLeft),
                        gilrs::Button::DPadRight => Some(PadButton::DpadRight),
                        _ => None,
                    };
                    if let Some(button) = mapped {
                        intents.extend(self.shell.controller_button(button, now));
                    }
                }
                _ => {}
            }
        }
        // The stick and the triggers are read as levels, not events, so a held throw
        // keeps streaming at the frame rate.
        if let Some((_, gamepad)) = pad.gamepads().next() {
            let axis = |axis: gilrs::Axis| f64::from(gamepad.value(axis));
            let (x, y) = (axis(gilrs::Axis::LeftStickX), axis(gilrs::Axis::LeftStickY));
            intents.extend(self.shell.controller_stick(x, y, now));
            let left = f64::from(
                gamepad
                    .button_data(gilrs::Button::LeftTrigger2)
                    .map_or(0.0, |data| data.value()),
            );
            let right = f64::from(
                gamepad
                    .button_data(gilrs::Button::RightTrigger2)
                    .map_or(0.0, |data| data.value()),
            );
            let dt = if self.last_pad_at.is_finite() {
                (now - self.last_pad_at).clamp(0.0, 0.1)
            } else {
                0.0
            };
            intents.extend(self.shell.controller_zoom(left, right, dt, now));
        }
        self.last_pad_at = now;
        for intent in intents {
            if let Intent::Send(command) = intent {
                self.link.send(command);
            }
        }
    }

    /// Reads the picture for the plates at about 15 Hz while any scope is on.
    fn sample_scopes(&mut self) {
        if !self.shell.scopes_wanted() {
            return;
        }
        let now = self.now();
        if now - self.last_scope_at < 1.0 / 15.0 {
            return;
        }
        if let Some(latest) = self.latest.as_ref() {
            self.shell
                .set_scope_samples(opc_monitor::scopes::ScopeSamples::read(latest));
            self.last_scope_at = now;
        }
    }

    /// Starts, stops or swaps the virtual camera to match the setting, and keeps the
    /// System tab's readout current about once a second.
    fn reconcile_vcam(&mut self, now: f64) {
        let wanted = self.shell.vcam_backend();
        let running = self.vcam.as_ref().map(VirtualCamera::backend);
        if running != wanted.as_ref() {
            if let Some(camera) = self.vcam.take() {
                camera.stop();
            }
            self.vcam = wanted.map(VirtualCamera::start);
            self.last_vcam_status_at = f64::NEG_INFINITY;
        }
        if now - self.last_vcam_status_at < 1.0 {
            return;
        }
        self.last_vcam_status_at = now;
        let line = self
            .vcam
            .as_ref()
            .map_or_else(|| "Off".to_string(), |camera| camera.status().line());
        self.shell.set_vcam_status(&line);
    }

    /// Hands the picture to the virtual camera at up to 30 frames a second: the feed
    /// on the viewfinder, the clip or the still in the player, nothing under the
    /// library. The chrome is never in it.
    fn feed_vcam(&mut self, picture: &opc_decode::Picture<'_>, now: f64) {
        let Some(camera) = self.vcam.as_ref() else {
            return;
        };
        if now - self.last_vcam_at < 1.0 / 30.0 {
            return;
        }
        if self.shell.screen() == Screen::Library {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let options = self.shell.vcam_grade_options();
        match renderer.render_picture(picture, (opc_vcam::WIDTH, opc_vcam::HEIGHT), options) {
            Ok(image) => {
                camera.offer(opc_vcam::Frame {
                    width: image.width,
                    height: image.height,
                    rgba: image.pixels,
                });
                self.last_vcam_at = now;
            }
            Err(error) => eprintln!("virtual camera frame failed: {error}"),
        }
    }

    fn draw(&mut self) {
        self.pump_camera();
        self.decode();
        let now = self.now();
        self.poll_pad(now);
        self.poll_component_job();
        self.reconcile_vcam(now);
        let intents = self.shell.tick(now);
        self.carry_out_quietly(intents, now);
        for command in self.media.tick(&mut self.shell, now) {
            self.link.send(command);
        }

        // A screen other than the viewfinder presents its own picture: black under the
        // library, the still in the viewer, the clip's frame in the player.
        let on_screen = self.shell.screen() != Screen::Viewfinder;
        let latest = if on_screen {
            Some(self.media.picture().clone())
        } else {
            self.latest.clone()
        };
        let (Some(renderer), Some(latest)) = (self.renderer.as_mut(), latest.as_ref()) else {
            // No picture yet. ControlFlow::Wait (set in about_to_wait) keeps the GPU
            // idle; nothing to present until the first frame arrives.
            return;
        };

        self.shell.set_source(latest.width, latest.height);
        if !on_screen && matches!(self.shell.phase(), Phase::Waiting | Phase::Recovering) {
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
                if on_screen {
                    self.media.note_presented(now);
                } else {
                    self.shell.note_presented(now);
                    self.link.note_presented();
                }
            }
            Ok(Presented::Rebuilt) => {}
            Err(error) => eprintln!("present failed: {error}"),
        }
        self.feed_vcam(&picture, now);
    }
}

/// winit's key to the shell's. Everything unmapped is dropped here rather than in the
/// shell, so the shell's table stays the whole story.
fn translate(key: &Key) -> Option<UiKey> {
    match key.as_ref() {
        Key::Named(NamedKey::Space) => Some(UiKey::Space),
        Key::Named(NamedKey::Escape) => Some(UiKey::Escape),
        Key::Named(NamedKey::Tab) => Some(UiKey::Tab),
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
                self.shell.set_renderer_name(renderer.device_name());
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
                self.shell.note_time(now);
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
                self.shell.note_time(now);
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
        if self.latest.is_some()
            || self.media.is_active()
            || self.shell.screen() != Screen::Viewfinder
        {
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
        shell: Shell::new()
            .with_grade(graded)
            .with_model(options.model_id)
            .with_saved_prefs(),
        media: {
            let mut media = MediaDriver::default();
            media.set_body(options.model_id);
            media
        },
        last_scope_at: f64::NEG_INFINITY,
        pad: gilrs::Gilrs::new()
            .map_err(|error| eprintln!("no game controller support: {error}"))
            .ok(),
        last_pad_at: f64::NEG_INFINITY,
        remote: options.remote,
        model_id: options.model_id,
        camera_id: options
            .model_id
            .map(|id| format!("model-{id:04x}"))
            .unwrap_or_else(|| "camera".to_string()),
        link,
        decoder: None,
        pending: Vec::new(),
        latest: None,
        lut: options.lut,
        false_color: None,
        still: options.still,
        take_still: false,
        pointer: (0.0, 0.0),
        pointer_control: false,
        started: Instant::now(),
        vcam: None,
        last_vcam_at: f64::NEG_INFINITY,
        last_vcam_status_at: f64::NEG_INFINITY,
        component_job: None,
    };
    view.start_component_job(opc_vcam::install::probe);
    view.shell.set_link_info(&format!(
        "Wi-Fi datalink · {}",
        options
            .remote
            .map_or_else(|| "camera default".to_string(), |addr| addr.to_string())
    ));
    if let Some(pad) = view.pad.as_ref() {
        if let Some((_, gamepad)) = pad.gamepads().next() {
            view.shell.set_gamepad(Some(gamepad.name().to_string()));
        }
    }
    {
        let folder = luts::custom_folder();
        let _ = std::fs::create_dir_all(&folder);
        view.shell.set_lut_menu(opc_monitor::LutMenu {
            builtin: opc_render::built_in_names(),
            custom: luts::list_custom(&folder),
            folder: folder.display().to_string(),
        });
    }
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
