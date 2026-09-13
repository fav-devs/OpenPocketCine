//! What the viewfinder decides, with nothing that can only happen in a window.
//!
//! Every press, drag and tick goes through here and comes out as `Intent`s the window
//! carries out. That split is the point: the whole operator-facing behaviour — which key
//! rolls, what a drag becomes, when the countdown fires, what the chrome says — is
//! decided by code a test can drive with a fake clock, no camera and no GPU.

use std::collections::VecDeque;
use std::time::Instant;

use opc_camera::{Command, Status};
use opc_chrome::{Chrome, ChromeIntent, ChromeState};
use opc_render::{letterbox, GradeOptions, Peaking, PeakingSense, Rgba, Zebra};
use opc_ui::{
    next_frame_rate, next_resolution, Action, Controls, Countdown, Drag, Fit, Hud, Key, Phase,
    Stick,
};

/// How long before a timed take starts rolling.
const COUNTDOWN: f64 = 3.0;
/// The stick is re-sent while held. The gimbal moves until it is told to stop, so this
/// is a keepalive, not the thing that makes it move.
const STICK_REPEAT: f64 = 0.2;
/// How long a committed tracking box stays on screen.
///
/// It is not kept: the camera does not report where the subject moved to, so a box left
/// on screen would stop being where the subject is and the operator would believe it.
const BOX_CONFIRM: f64 = 1.5;
/// Frames older than this stop counting towards the rate shown.
const FPS_WINDOW: f64 = 1.0;
const ZOOM_MIN: f64 = 1.0;
const ZOOM_MAX: f64 = 6.0;


/// What the window is asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Put this on the wire.
    Send(Command),
    /// Write the picture on screen to a file.
    Still,
    /// Close.
    Quit,
}

/// What a finger did — winit's touch phases, without winit, so the rule about which
/// finger owns a drag can be checked without a touchscreen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    /// The system took the gesture, or a palm landed.
    Cancelled,
}

/// Assists the operator has switched on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggles {
    pub zebra: bool,
    pub peaking: bool,
    pub grade: bool,
    pub mirror: bool,
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

    fn names(self) -> Vec<&'static str> {
        [
            ("LUT", self.grade),
            ("ZEB", self.zebra),
            ("PEAK", self.peaking),
            ("MIR", self.mirror),
        ]
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| name)
        .collect()
    }
}

/// The viewfinder's state.
#[derive(Debug)]
pub struct Shell {
    controls: Controls,
    stick: Stick,
    hud: Hud,
    toggles: Toggles,
    chrome_renderer: Option<Chrome>,
    /// A box being dragged out right now.
    drag: Option<Drag>,
    /// A box already sent, and when it stops being drawn.
    committed: Option<((f64, f64, f64, f64), f64)>,
    /// The finger drawing the box, if one is. A second finger must not take over a box
    /// somebody is halfway through drawing.
    finger: Option<u64>,
    /// A touch that landed in a control. It cannot become a tracking drag.
    control_finger: Option<u64>,
    window: (u32, u32),
    /// Intents fired by Slint controls (buttons, slider) since last tick.
    chrome_pending_intents: Vec<Intent>,
    source: Option<(u32, u32)>,
    /// Tracking boxes are numbered so the camera can tell one request from the next.
    next_track_id: u16,
    chrome_visible: bool,
    /// The window has to be told to load or drop the cube, which is not a per-frame job.
    lut_pending: Option<bool>,
    stick_sent_at: f64,
    presented: VecDeque<f64>,
    /// The rasterised chrome, kept until something it draws changes.
    chrome: Option<Rgba>,
    chrome_stale: bool,
    /// The second the countdown last showed, so a ticking number redraws and a still one
    /// does not.
    drawn_second: Option<u32>,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    pub fn new() -> Self {
        let chrome_renderer = Chrome::new(Instant::now())
            .map_err(|e| eprintln!("Slint chrome init failed: {e}"))
            .ok();
        Self {
            controls: Controls::new(),
            stick: Stick::default(),
            hud: Hud::default(),
            toggles: Toggles::default(),
            chrome_renderer,
            drag: None,
            committed: None,
            finger: None,
            control_finger: None,
            window: (0, 0),
            chrome_pending_intents: Vec::new(),
            source: None,
            next_track_id: 1,
            chrome_visible: true,
            lut_pending: None,
            stick_sent_at: f64::NEG_INFINITY,
            presented: VecDeque::new(),
            chrome: None,
            chrome_stale: true,
            drawn_second: None,
        }
    }

    /// Starts with the cube already on, for `--lut`.
    pub fn with_grade(mut self, graded: bool) -> Self {
        self.toggles.grade = graded;
        self
    }

    pub fn toggles(&self) -> Toggles {
        self.toggles
    }

    pub fn grade_options(&self) -> GradeOptions {
        self.toggles.options()
    }

    /// Whether the cube should be loaded or dropped, once. The window does that between
    /// frames because it waits for the device to go idle.
    pub fn take_lut_change(&mut self) -> Option<bool> {
        self.lut_pending.take()
    }

    pub fn phase(&self) -> &Phase {
        &self.hud.phase
    }

    pub fn set_phase(&mut self, phase: Phase) {
        if self.hud.phase != phase {
            self.hud.phase = phase;
            self.chrome_stale = true;
        }
    }

    pub fn status(&self) -> &Status {
        &self.hud.status
    }

    /// Takes the camera's word for what it is set to.
    pub fn set_status(&mut self, status: Status) {
        // The body is the source of truth for zoom: somebody may have turned the ring.
        if let Some(hundredths) = status.zoom_hundredths {
            self.controls.set_zoom(f64::from(hundredths) / 100.0);
        }
        self.hud.status = status;
        self.chrome_stale = true;
    }

    pub fn set_window(&mut self, width: u32, height: u32) {
        if self.window != (width, height) {
            self.window = (width, height);
            self.chrome_stale = true;
            self.chrome = None;
        }
    }

    pub fn set_source(&mut self, width: u32, height: u32) {
        if self.source != Some((width, height)) {
            self.source = Some((width, height));
            self.chrome_stale = true;
        }
    }

    /// Where the picture sits in the window, which is also where a click has to be read.
    pub fn fit(&self) -> Fit {
        let Some(source) = self.source else {
            return Fit {
                x: 0.0,
                y: 0.0,
                width: f64::from(self.window.0),
                height: f64::from(self.window.1),
            };
        };
        // The same rectangle the blit draws into, from the same arithmetic.
        let (x, y, width, height) = letterbox(source, self.window);
        Fit {
            x: f64::from(x),
            y: f64::from(y),
            width: f64::from(width),
            height: f64::from(height),
        }
    }

    /// A picture reached the screen. Drives the rate shown in the chrome.
    pub fn note_presented(&mut self, now: f64) {
        self.presented.push_back(now);
        while self
            .presented
            .front()
            .is_some_and(|at| now - at > FPS_WINDOW)
        {
            self.presented.pop_front();
        }
        let rate = self.presented.len() as u32;
        if self.hud.fps != rate {
            self.hud.fps = rate;
            self.chrome_stale = true;
        }
    }

    /// A key went down.
    pub fn press(&mut self, key: Key, now: f64) -> Vec<Intent> {
        if self.stick.set(key, true) {
            self.stick_sent_at = now;
            return vec![Intent::Send(self.stick.command())];
        }
        let Some(action) = self.controls.press(key) else {
            return Vec::new();
        };
        self.act(action, now)
    }

    /// A key came up. Only the stick cares.
    pub fn release(&mut self, key: Key, now: f64) -> Vec<Intent> {
        if self.stick.set(key, false) {
            self.stick_sent_at = now;
            return vec![Intent::Send(self.stick.command())];
        }
        Vec::new()
    }

    fn act(&mut self, action: Action, now: f64) -> Vec<Intent> {
        self.chrome_stale = true;
        match action {
            Action::Send(command) => vec![Intent::Send(command)],
            Action::Still => vec![Intent::Still],
            Action::Quit => vec![Intent::Quit],
            Action::ToggleTimer => {
                // Pressing it again while it counts is a cancel, which is what an
                // operator reaches for when the shot is not ready.
                self.hud.countdown = match self.hud.countdown {
                    Some(_) => None,
                    None => Some(Countdown::start(now, COUNTDOWN)),
                };
                Vec::new()
            }
            Action::ToggleZebra => {
                self.toggles.zebra = !self.toggles.zebra;
                self.refresh_assists();
                Vec::new()
            }
            Action::TogglePeaking => {
                self.toggles.peaking = !self.toggles.peaking;
                self.refresh_assists();
                Vec::new()
            }
            Action::ToggleMirror => {
                self.toggles.mirror = !self.toggles.mirror;
                self.refresh_assists();
                Vec::new()
            }
            Action::ToggleGrade => {
                self.toggles.grade = !self.toggles.grade;
                self.lut_pending = Some(self.toggles.grade);
                self.refresh_assists();
                Vec::new()
            }
            Action::ToggleChrome => {
                self.chrome_visible = !self.chrome_visible;
                Vec::new()
            }
            Action::ClearTracking => {
                self.drag = None;
                self.committed = None;
                self.hud.drag = None;
                vec![Intent::Send(Command::TrackClear)]
            }
            Action::CycleResolution | Action::CycleFrameRate => {
                let status = &self.hud.status;
                let current = status.video_resolution.zip(status.video_frame_rate);
                let stepped = if matches!(action, Action::CycleResolution) {
                    next_resolution(&status.available_formats, current)
                } else {
                    next_frame_rate(&status.available_formats, current)
                };
                // Nothing to step to sends nothing: a command the camera would echo
                // back unchanged is a command that only costs a round trip.
                stepped
                    .map(|(resolution, frame_rate)| {
                        Intent::Send(Command::SetVideoFormat {
                            resolution,
                            frame_rate,
                        })
                    })
                    .into_iter()
                    .collect()
            }
        }
    }

    fn refresh_assists(&mut self) {
        self.hud.assists = self.toggles.names();
    }

    /// The pointer went down. Outside the picture it is not the start of a box.
    pub fn pointer_down(&mut self, x: f64, y: f64) {
        let Some(point) = self.fit().normalise(x, y) else {
            return;
        };
        self.drag = Some(Drag::start(point, self.toggles.mirror));
        self.committed = None;
        self.hud.drag = None;
        self.chrome_stale = true;
    }

    /// The pointer moved while down. A drag that leaves the picture keeps its corner on
    /// the edge rather than ending, so a box can be pulled right out to the frame.
    pub fn pointer_moved(&mut self, x: f64, y: f64) {
        if self.drag.is_none() {
            return;
        }
        let fit = self.fit();
        if fit.width <= 0.0 || fit.height <= 0.0 {
            return;
        }
        let u = ((x - fit.x) / fit.width).clamp(0.0, 1.0);
        let v = ((y - fit.y) / fit.height).clamp(0.0, 1.0);
        let drag = self.drag.as_mut().expect("a drag was just checked");
        drag.extend((u, v));
        self.hud.drag = Some(drag.rectangle());
        self.chrome_stale = true;
    }

    /// The pointer came up. A real box is sent; a click is not.
    pub fn pointer_up(&mut self, x: f64, y: f64, now: f64) -> Vec<Intent> {
        self.pointer_moved(x, y);
        let Some(drag) = self.drag.take() else {
            return Vec::new();
        };
        self.chrome_stale = true;
        let Some(command) = drag.command(self.next_track_id) else {
            // A click, not a drag. Leaving whatever the camera is already following
            // alone is safer than clearing it by accident.
            self.hud.drag = None;
            return Vec::new();
        };
        self.next_track_id = self.next_track_id.wrapping_add(1).max(1);
        self.committed = Some((drag.rectangle(), now + BOX_CONFIRM));
        vec![Intent::Send(command)]
    }

    /// A finger touched, moved, or left the screen.
    ///
    /// A touchscreen does not synthesise mouse clicks once the window is registered for
    /// touch, so this is the only way a finger reaches the picture.
    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f64, y: f64, now: f64) -> Vec<Intent> {
        let mine = self.finger == Some(id);
        let control_mine = self.control_finger == Some(id);
        match phase {
            TouchPhase::Started if self.finger.is_none() && self.control_finger.is_none() => {
                if let Some(intents) = self.control_down(x, y, now) {
                    self.control_finger = Some(id);
                    return intents;
                }
                self.pointer_down(x, y);
                // Claimed only if a box actually started. A finger that landed on a
                // letterbox bar must not lock out the next one that lands on the shot.
                if self.drag.is_some() {
                    self.finger = Some(id);
                }
            }
            TouchPhase::Moved if control_mine => return self.control_moved(x, y),
            TouchPhase::Ended if control_mine => {
                self.control_finger = None;
                return self.control_up(x, y, now);
            }
            TouchPhase::Cancelled if control_mine => {
                self.control_finger = None;
                return self.control_cancel();
            }
            TouchPhase::Moved if mine => self.pointer_moved(x, y),
            TouchPhase::Ended if mine => {
                self.finger = None;
                return self.pointer_up(x, y, now);
            }
            TouchPhase::Cancelled if mine => {
                self.finger = None;
                self.pointer_cancel();
            }
            _ => {}
        }
        Vec::new()
    }

    /// The drag was taken away rather than finished — a palm landing on a touchscreen,
    /// or the system claiming the gesture for itself.
    ///
    /// The box is abandoned, never committed: pointing the camera at whatever a finger
    /// happened to be over is worse than not tracking at all.
    pub fn pointer_cancel(&mut self) {
        if self.drag.take().is_some() {
            self.hud.drag = None;
            self.chrome_stale = true;
        }
    }

    fn controls_enabled(&self) -> bool {
        matches!(self.hud.phase, Phase::Live)
    }

    /// Forward a move to Slint even when the pointer isn't in a control zone.
    /// This keeps hover states and drag-in-progress updates working correctly.
    pub fn slint_pointer_moved(&self, x: f64, y: f64) {
        if let Some(cr) = &self.chrome_renderer {
            cr.pointer_moved(x as f32, y as f32);
        }
    }

    /// Whether this point belongs to a Slint control rather than the tracking-box area.
    pub fn is_control(&self, x: f64, y: f64) -> bool {
        if !self.chrome_visible {
            return false;
        }
        if let Some(cr) = &self.chrome_renderer {
            cr.is_over_control(x, y, self.window.0, self.window.1)
        } else {
            false
        }
    }

    /// Pointer pressed in a control zone: forward to Slint.
    /// Returns `Some([])` so the caller knows it was claimed (even if no intent fired yet).
    pub fn control_down(&mut self, x: f64, y: f64, _now: f64) -> Option<Vec<Intent>> {
        if !self.chrome_visible {
            return None;
        }
        if !self.is_control(x, y) {
            return None;
        }
        if let Some(cr) = &self.chrome_renderer {
            cr.pointer_pressed(x as f32, y as f32);
        }
        self.chrome_stale = true;
        Some(Vec::new())
    }

    /// Pointer moved while a Slint control is held.
    pub fn control_moved(&mut self, x: f64, y: f64) -> Vec<Intent> {
        if let Some(cr) = &self.chrome_renderer {
            cr.pointer_moved(x as f32, y as f32);
        }
        Vec::new()
    }

    /// Pointer released over a control zone.
    pub fn control_up(&mut self, x: f64, y: f64, _now: f64) -> Vec<Intent> {
        if let Some(cr) = &self.chrome_renderer {
            cr.pointer_released(x as f32, y as f32);
        }
        Vec::new()
    }

    /// Focus lost or gesture cancelled — no in-flight Slint gesture to clean up.
    pub fn control_cancel(&mut self) -> Vec<Intent> {
        Vec::new()
    }

    /// The clock moved on: fires the countdown, keeps the stick alive, and
    /// returns any intents fired by Slint controls since last tick.
    pub fn tick(&mut self, now: f64) -> Vec<Intent> {
        let mut intents = Vec::new();
        // Drain intents queued by Slint button callbacks.
        intents.extend(self.chrome_pending_intents.drain(..));
        if self.hud.countdown_fired(now) {
            self.hud.countdown = None;
            self.chrome_stale = true;
            intents.push(Intent::Send(Command::RecordStart));
        }
        if !self.stick.is_resting() && now - self.stick_sent_at >= STICK_REPEAT {
            self.stick_sent_at = now;
            intents.push(Intent::Send(self.stick.command()));
        }
        if let Some((_, until)) = self.committed {
            if now >= until {
                self.committed = None;
                self.hud.drag = None;
                self.chrome_stale = true;
            }
        }
        intents
    }

    /// The chrome for this frame, or `None` when the operator has hidden it or there is
    /// nothing to draw on.
    pub fn chrome(&mut self, now: f64) -> Option<&Rgba> {
        if !self.chrome_visible || self.window.0 == 0 || self.window.1 == 0 {
            return None;
        }
        let second = self
            .hud
            .countdown
            .filter(|countdown| !countdown.is_done(now))
            .map(|countdown| countdown.remaining(now));
        if second != self.drawn_second {
            self.drawn_second = second;
            self.chrome_stale = true;
        }
        if self.chrome_stale || self.chrome.is_none() {
            self.hud.fit = Some(self.fit());
            if let Some((rectangle, _)) = self.committed {
                self.hud.drag = Some(rectangle);
            }

            let mut canvas = if let Some(cr) = self.chrome_renderer.as_mut() {
                let chips = self.hud.top_chips();
                let chip = |i: usize| chips.get(i).cloned().unwrap_or_default();
                let link_state = self.hud.connection_chip();
                let rec_elapsed = self.hud.status.elapsed_label();
                let bottom_line = self.hud.bottom_line();
                let state = ChromeState {
                    phase: &self.hud.phase,
                    chip1: chip(0),
                    chip2: chip(1),
                    chip3: chip(2),
                    chip4: chip(3),
                    chip5: chip(4),
                    link_state,
                    is_recording: self.hud.status.is_recording,
                    rec_elapsed,
                    bottom_line,
                    zoom: self.controls.zoom() as f32,
                };
                let canvas = cr.render(&state, self.window.0, self.window.1);
                // Process any intents fired by Slint callbacks during this render.
                for intent in cr.drain_intents() {
                    let shell_intent = match intent {
                        ChromeIntent::RecordToggle => {
                            if self.hud.status.is_recording {
                                Intent::Send(Command::RecordStop)
                            } else {
                                Intent::Send(Command::RecordStart)
                            }
                        }
                        ChromeIntent::TakeStill => Intent::Send(Command::ShootPhoto),
                        ChromeIntent::GimbalFlip => Intent::Send(Command::GimbalFlip),
                        ChromeIntent::GimbalRecenter => Intent::Send(Command::GimbalRecenter),
                        ChromeIntent::ZoomSet(v) => {
                            let zoom = (v as f64).clamp(ZOOM_MIN, ZOOM_MAX);
                            self.controls.set_zoom(zoom);
                            self.chrome_stale = true;
                            Intent::Send(Command::ZoomFactor(zoom))
                        }
                    };
                    self.chrome_pending_intents.push(shell_intent);
                }
                canvas
            } else {
                // Fallback: old CPU canvas (Slint unavailable).
                self.hud.draw(self.window.0, self.window.1, now)
            };

            // Draw tracking box on top.
            if let Some((x, y, bw, bh)) = self.hud.drag {
                let fit = self.hud.fit.unwrap_or(opc_ui::Fit {
                    x: 0.0,
                    y: 0.0,
                    width: f64::from(self.window.0),
                    height: f64::from(self.window.1),
                });
                canvas.stroke(
                    (fit.x + x * fit.width) as i64,
                    (fit.y + y * fit.height) as i64,
                    (bw * fit.width) as u32,
                    (bh * fit.height) as u32,
                    2,
                    opc_ui::canvas::TRACKING,
                );
            }

            self.chrome = Some(Rgba {
                width: canvas.width,
                height: canvas.height,
                pixels: canvas.pixels,
            });
            self.chrome_stale = false;
        }
        self.chrome.as_ref()
    }

    /// Whether the chrome is being drawn at all.
    pub fn chrome_visible(&self) -> bool {
        self.chrome_visible
    }

}
