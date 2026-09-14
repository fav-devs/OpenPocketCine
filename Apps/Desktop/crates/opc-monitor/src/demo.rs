//! Offline UI demo — no camera, no Swift core required.
//!
//! `opc-monitor demo` opens the viewfinder window pre-loaded with synthetic camera
//! status and a slate-gray test frame, then steps through the phases an operator would
//! see on a real shoot. Useful for verifying the chrome without a physical camera.

use std::time::Instant;

use opc_camera::Status;
use opc_decode::{OwnedPicture, Picture};
use opc_render::{FeedRenderer, GradeOptions, Presented};
use opc_ui::{Key as UiKey, Phase};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use opc_monitor::shell::{Intent, Shell, TouchPhase as Finger};

// ── Timeline ────────────────────────────────────────────────────────────────

/// One step in the demo sequence.
struct Step {
    phase: Phase,
    /// Seconds to hold this step before advancing.
    hold: f64,
}

fn timeline() -> Vec<Step> {
    vec![
        Step {
            phase: Phase::Finding,
            hold: 2.0,
        },
        Step {
            phase: Phase::Pairing {
                needs_approval: false,
            },
            hold: 1.5,
        },
        Step {
            phase: Phase::Pairing {
                needs_approval: true,
            },
            hold: 2.0,
        },
        Step {
            phase: Phase::Joining,
            hold: 1.5,
        },
        Step {
            phase: Phase::Waiting,
            hold: 1.0,
        },
        // Live — idle
        Step {
            phase: Phase::Live,
            hold: 3.0,
        },
        // Live — recording (status flipped below in `apply_status`)
        Step {
            phase: Phase::Live,
            hold: 5.0,
        },
        Step {
            phase: Phase::Recovering,
            hold: 2.0,
        },
        Step {
            phase: Phase::Failed("DEMO: camera went away".to_string()),
            hold: 3.0,
        },
    ]
}

/// Fake camera status for a given step index.
fn status_for(step: usize) -> Status {
    let recording = step == 6;
    Status {
        iso: Some(400),
        shutter_denominator: Some(50),
        ev_thirds: Some(-3),
        white_balance_kelvin: Some(5600),
        zoom_hundredths: Some(100),
        fps: Some(24),
        battery_percent: Some(73),
        storage_free_mb: 32_768,
        storage_total_mb: 64_000,
        is_recording: recording,
        record_elapsed: if recording { 42 } else { 0 },
        ..Status::default()
    }
}

// ── Synthetic frame ──────────────────────────────────────────────────────────

/// Builds a 1920×1080 YUV420 frame with a horizontal luma gradient and neutral chroma,
/// giving a pleasant dark-to-mid-gray sweep across the screen.
fn make_frame() -> OwnedPicture {
    let width: u32 = 1920;
    let height: u32 = 1080;
    let chroma_w = width.div_ceil(2) as usize;
    let chroma_h = height.div_ceil(2) as usize;

    let mut luma = vec![0u8; (width * height) as usize];
    for row in 0..height as usize {
        for col in 0..width as usize {
            // Bright daylight gradient 110–200, light vignette — simulates a real scene
            // so the dark overlay scrims are visibly distinct from the video.
            let h_frac = col as f32 / (width - 1) as f32;
            let v_frac = row as f32 / (height - 1) as f32;
            let cx = (h_frac - 0.5).abs();
            let cy = (v_frac - 0.5).abs();
            let vignette = 1.0 - (cx * cx + cy * cy).sqrt() * 0.35;
            let base = 110.0 + h_frac * 90.0;
            luma[row * width as usize + col] = (base * vignette).clamp(16.0, 235.0) as u8;
        }
    }
    let chroma_blue = vec![128u8; chroma_w * chroma_h];
    let chroma_red = vec![128u8; chroma_w * chroma_h];

    let picture = Picture {
        width,
        height,
        is_keyframe: true,
        luma: &luma,
        chroma_blue: &chroma_blue,
        chroma_red: &chroma_red,
        luma_stride: width as usize,
        chroma_stride: chroma_w,
    };
    OwnedPicture::copy_from(&picture)
}

// ── Window handler ───────────────────────────────────────────────────────────

struct Demo {
    renderer: Option<FeedRenderer>,
    window: Option<Window>,
    shell: Shell,
    frame: OwnedPicture,
    started: Instant,
    timeline: Vec<Step>,
    step: usize,
    step_started: f64,
    pointer: (f64, f64),
    pointer_control: bool,
}

impl Demo {
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn advance_timeline(&mut self, now: f64) {
        let elapsed = now - self.step_started;
        if elapsed >= self.timeline[self.step].hold {
            self.step = (self.step + 1) % self.timeline.len();
            self.step_started = now;
            let phase = self.timeline[self.step].phase.clone();
            self.shell.set_phase(phase);
            self.shell.set_status(status_for(self.step));
        }
    }

    fn carry_out(&mut self, intents: Vec<Intent>, event_loop: &ActiveEventLoop) {
        for intent in intents {
            match intent {
                Intent::Quit => event_loop.exit(),
                Intent::ToggleFullscreen => {
                    if let Some(window) = self.window.as_ref() {
                        let wanted = window.fullscreen().is_none();
                        window.set_fullscreen(wanted.then_some(Fullscreen::Borderless(None)));
                    }
                }
                Intent::Send(_)
                | Intent::Still
                | Intent::Media(_)
                | Intent::Reconnect
                | Intent::Diagnostics
                | Intent::ComponentInstall
                | Intent::ComponentRemove
                | Intent::OpenUrl(_) => {}
            }
        }
    }

    fn draw(&mut self) {
        let now = self.now();
        self.advance_timeline(now);
        let intents = self.shell.tick(now);
        for intent in intents {
            if let Intent::Quit = intent {
                // tick shouldn't quit, but be safe
            }
        }

        let (Some(renderer), _) = (self.renderer.as_mut(), ()) else {
            return;
        };

        let picture = self.frame.picture();
        self.shell.set_source(picture.width, picture.height);

        match self.shell.chrome(now) {
            Some(chrome) => renderer.set_overlay(chrome),
            None => renderer.clear_overlay(),
        }

        let options = GradeOptions::default();
        match renderer.present(&picture, options) {
            Ok(Presented::Shown) => self.shell.note_presented(now),
            Ok(Presented::Rebuilt) => {}
            Err(error) => eprintln!("demo present failed: {error}"),
        }
    }
}

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

impl ApplicationHandler for Demo {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("OpenPocketCine — UI Demo")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => w,
            Err(error) => {
                eprintln!("could not open a demo window: {error}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        let handles = window
            .display_handle()
            .and_then(|d| window.window_handle().map(|h| (d.as_raw(), h.as_raw())));
        let Ok((display, raw_window)) = handles else {
            eprintln!("demo window gave no handles");
            event_loop.exit();
            return;
        };
        let renderer = unsafe {
            FeedRenderer::for_window(display, raw_window, size.width.max(1), size.height.max(1))
        };
        match renderer {
            Ok(r) => {
                println!("demo drawing on {}", r.device_name());
                self.renderer = Some(r);
            }
            Err(error) => {
                eprintln!("could not start demo pipeline: {error}");
                event_loop.exit();
                return;
            }
        }
        self.shell.set_window(size.width.max(1), size.height.max(1));
        // Seed the first step.
        self.shell.set_phase(self.timeline[0].phase.clone());
        self.shell.set_status(status_for(0));
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let now = self.now();
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let (w, h) = (size.width.max(1), size.height.max(1));
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(w, h);
                }
                self.shell.set_window(w, h);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let Some(key) = translate(&event.logical_key) else {
                    return;
                };
                let intents = match event.state {
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
                    // Always forward moves to Slint for hover states even outside control zones.
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
                let intents =
                    self.shell
                        .touch(touch.id, finger, touch.location.x, touch.location.y, now);
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Always poll — the demo frame does not change but the timeline advances on time.
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

/// Opens a demo viewfinder with synthetic status and no camera required.
pub fn run() -> Result<(), String> {
    let timeline = timeline();
    let mut demo = Demo {
        renderer: None,
        window: None,
        shell: Shell::new(),
        frame: make_frame(),
        started: Instant::now(),
        step: 0,
        step_started: 0.0,
        timeline,
        pointer: (0.0, 0.0),
        pointer_control: false,
    };
    demo.shell.set_phase(Phase::Finding);

    let event_loop = EventLoop::new().map_err(|e| format!("no window system: {e}"))?;
    event_loop
        .run_app(&mut demo)
        .map_err(|e| format!("demo closed unexpectedly: {e}"))
}
