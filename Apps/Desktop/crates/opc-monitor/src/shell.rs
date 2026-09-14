//! What the viewfinder decides, with nothing that can only happen in a window.
//!
//! Every press, drag and tick goes through here and comes out as `Intent`s the window
//! carries out. That split is the point: the whole operator-facing behaviour — which key
//! rolls, what a drag becomes, when the countdown fires, what the chrome says — is
//! decided by code a test can drive with a fake clock, no camera and no GPU.

use std::collections::VecDeque;
use std::time::Instant;

use opc_camera::{Command, Status};
use opc_chrome::{AssistChip, Chrome, ChromeIntent, ChromeState, Overlays, Screen};
use opc_media::MediaFile;

use crate::assists::{
    guide_rect, AssistOptions, AssistTool, ZebraSteps, ZEBRA_HIGHLIGHT_STEPS, ZEBRA_MIDTONE_STEPS,
};
use crate::library::{Library, MediaAction, Player};
use crate::luts::{LutChoice, LutMenu};
use crate::moves::{MoveEngine, Program, Waypoint};
use crate::sheets::{self, Pick, Prefs, SheetKind, Slot};
use opc_render::{letterbox, AssistScalars, FalseColorScale, GradeOptions, Peaking, Rgba, Zebra};
use opc_ui::{
    next_frame_rate, next_resolution, Action, Controls, Countdown, Drag, Fit, Hud, Key, Phase,
    Stick,
};

/// How long before a timed take starts rolling.
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
    /// Toggle the window between fullscreen and windowed.
    ToggleFullscreen,
    /// Something for the media browser: a fetch, a screen change, the player.
    Media(MediaAction),
}

/// The gimbal's live mode, as commanded. The body's GET cannot tell FPV from Tilt
/// locked, so the shell keeps what it last asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GimbalMode {
    Follow,
    TiltLocked,
    Fpv,
}

impl GimbalMode {
    /// The SET frames for a mode, in the order the mobile shells send them.
    pub fn commands(self) -> Vec<Command> {
        match self {
            Self::Follow => vec![Command::GimbalFollow, Command::GimbalTiltLock(0)],
            Self::TiltLocked => vec![Command::GimbalFollow, Command::GimbalTiltLock(1)],
            Self::Fpv => vec![Command::GimbalFpv],
        }
    }

    /// Mimo's order when the FOLLOW button cycles.
    pub fn next(self) -> Self {
        match self {
            Self::Follow => Self::TiltLocked,
            Self::TiltLocked => Self::Fpv,
            Self::Fpv => Self::Follow,
        }
    }
}

/// Shooting-mode codes behind the mode strip, by [`opc_chrome::MODES`] index. `None` is a
/// mode the strip shows but this shell cannot select (Pano, Livestream).
const MODE_CODES: [Option<u8>; 7] = [
    Some(0x02), // TIMELAPSE
    Some(0x00), // SLOWMOTION
    Some(0x28), // LOW-LIGHT (SuperNight)
    Some(0x01), // VIDEO
    Some(0x17), // PHOTO (Pocket 4; Nano's 0x05 reads the same)
    None,       // PANO
    None,       // LIVESTREAM
];

/// The strip index for a shooting-mode code the body reported. Unknown codes read as
/// VIDEO rather than moving the highlight somewhere the operator did not tap.
fn mode_index(code: Option<i32>) -> usize {
    match code {
        Some(0x02) => 0,
        Some(0x00) => 1,
        Some(0x28) => 2,
        Some(0x17) | Some(0x05) => 4,
        _ => 3,
    }
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
    pub false_color: bool,
    pub grid: bool,
    pub guides: bool,
    pub cross: bool,
}

impl Toggles {
    /// What the feed pass paints. The zebra thresholds are the operator's IRE put on
    /// the feed's axis by the core; without the core linked they are read as a plain
    /// fraction, which is right for Rec.709 and generous for log.
    fn options(self, assists: &AssistOptions, scalars: Option<AssistScalars>) -> GradeOptions {
        let zebra = assists.zebra;
        let (highlight, midtone) = match scalars {
            Some(scalars) => (scalars.highlight, (scalars.midtone, scalars.midtone_half)),
            None => (
                zebra.highlight_ire / 100.0,
                (zebra.midtone_ire / 100.0, 0.05),
            ),
        };
        GradeOptions {
            mirror: self.mirror,
            zebra: self.zebra.then(|| Zebra {
                highlight: zebra.highlight_on.then_some(highlight),
                midtone: zebra.midtone_on.then_some(midtone),
                highlight_color: zebra.highlight_color.rgba(),
                midtone_color: zebra.midtone_color.rgba(),
            }),
            peaking: self.peaking.then(|| Peaking {
                sense: assists.peaking_sense,
                color: assists.peaking_color.rgba(),
            }),
            false_color: self.false_color,
            ..GradeOptions::default()
        }
    }

    fn names(self) -> Vec<&'static str> {
        [
            ("LUT", self.grade),
            ("ZEB", self.zebra),
            ("PEAK", self.peaking),
            ("FALSE", self.false_color),
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
    /// The window has to be told to load or drop cubes, which is not a per-frame job.
    lut_pending: VecDeque<LutRequest>,
    /// Every assist tool's options.
    assists: AssistOptions,
    /// The assist toolbar is showing under the top bar.
    assist_bar: bool,
    /// Which false-colour lattices the window was last asked to load.
    false_color_key: Option<FalseColorKey>,
    stick_sent_at: f64,
    presented: VecDeque<f64>,
    /// The rasterised chrome, kept until something it draws changes.
    chrome: Option<Rgba>,
    chrome_stale: bool,
    /// The second the countdown last showed, so a ticking number redraws and a still one
    /// does not.
    drawn_second: Option<u32>,
    gimbal_mode: GimbalMode,
    /// The on-screen joystick is being held, so a cancelled gesture must rest the stick.
    pad_held: bool,
    /// The sheet over the picture, if one is open, and which settings tab it shows.
    sheet: Option<SheetKind>,
    sheet_tab: usize,
    prefs: Prefs,
    /// The body's model id for commands that encode per model, or -1 when unknown.
    model_id: i32,
    screen: Screen,
    library: Library,
    player: Option<Player>,
    /// Delete and favourite carry a running index the camera does not police.
    media_counter: u32,
    /// The stick's ease, when the ramp is on: the filtered throw and its clock.
    ramp: opc_ui::RampFilter,
    ramp_ticked_at: f64,
    /// The on-screen pad's throw while it is held, right and up positive.
    pad_target: (f64, f64),
    lut_menu: LutMenu,
    lut_choice: LutChoice,
    /// Programmed moves: the points, the engine while a take runs, and the clock
    /// that says how fresh the last attitude is.
    program: Program,
    move_engine: Option<MoveEngine>,
    move_countdown: Option<Countdown>,
    attitude_seq: u32,
    attitude_at: f64,
    last_now: f64,
    move_ticked_at: f64,
}

/// What the window must do about the cube, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LutRequest {
    /// Keep the loaded cube and switch it on or off.
    Toggle(bool),
    /// Load this cube and switch it on (or off, for `Off`).
    Load(LutChoice),
    /// Load the false-colour lattices for this scale, colour mode and ISO, or drop them.
    FalseColor(Option<FalseColorKey>),
}

/// Which false-colour lattices the window should hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FalseColorKey {
    pub scale: FalseColorScale,
    pub color_mode: i32,
    pub iso: i32,
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
            lut_pending: VecDeque::new(),
            assists: AssistOptions::default(),
            assist_bar: false,
            false_color_key: None,
            stick_sent_at: f64::NEG_INFINITY,
            presented: VecDeque::new(),
            chrome: None,
            chrome_stale: true,
            gimbal_mode: GimbalMode::Follow,
            pad_held: false,
            sheet: None,
            sheet_tab: 0,
            prefs: Prefs::default(),
            model_id: -1,
            screen: Screen::Viewfinder,
            library: Library::default(),
            player: None,
            media_counter: 0,
            ramp: opc_ui::RampFilter::default(),
            ramp_ticked_at: 0.0,
            pad_target: (0.0, 0.0),
            lut_menu: LutMenu::default(),
            lut_choice: LutChoice::Off,
            program: Program::default(),
            move_engine: None,
            move_countdown: None,
            attitude_seq: 0,
            attitude_at: f64::NEG_INFINITY,
            last_now: 0.0,
            move_ticked_at: 0.0,
            drawn_second: None,
        }
    }

    /// Starts with the cube already on, for `--lut`.
    pub fn with_grade(mut self, graded: bool) -> Self {
        self.toggles.grade = graded;
        self
    }

    pub fn with_model(mut self, model_id: Option<i32>) -> Self {
        self.set_model(model_id);
        self
    }

    pub fn toggles(&self) -> Toggles {
        self.toggles
    }

    pub fn grade_options(&self) -> GradeOptions {
        self.toggles.options(&self.assists, self.scalars())
    }

    /// Every tool's options, as the sheets set them.
    pub fn assists(&self) -> AssistOptions {
        self.assists
    }

    /// The assist toolbar is showing.
    pub fn assist_bar_open(&self) -> bool {
        self.assist_bar
    }

    /// Whether `tool` is on, as its chip shows it.
    pub fn tool_on(&self, tool: AssistTool) -> bool {
        sheets::tool_on(tool, self.toggles)
    }

    /// Whether a cube should be loaded or dropped, once each. The window does that
    /// between frames because it waits for the device to go idle.
    pub fn take_lut_change(&mut self) -> Option<LutRequest> {
        self.lut_pending.pop_front()
    }

    /// The body's colour mode and ISO, as the core's assist calls want them.
    fn color_axis(&self) -> (i32, i32) {
        let status = &self.hud.status;
        (
            i32::from(status.color_mode.unwrap_or(0x3F)),
            status.iso.unwrap_or(0),
        )
    }

    /// The zebra thresholds on the feed's axis, from the core. `None` without it.
    #[cfg(opc_core_linked)]
    fn scalars(&self) -> Option<AssistScalars> {
        let (color_mode, iso) = self.color_axis();
        let zebra = self.assists.zebra;
        opc_render::assist_scalars(color_mode, iso, zebra.highlight_ire, zebra.midtone_ire)
    }

    #[cfg(not(opc_core_linked))]
    fn scalars(&self) -> Option<AssistScalars> {
        None
    }

    /// Where each zebra chip lands on the feed, for the sheet's 0–255 labels.
    #[cfg(opc_core_linked)]
    fn zebra_steps(&self) -> ZebraSteps {
        let (color_mode, iso) = self.color_axis();
        let native = |ire: f32| {
            opc_render::assist_scalars(color_mode, iso, ire, ire)
                .map_or(ire / 100.0, |scalars| scalars.highlight)
        };
        ZebraSteps {
            highlight: ZEBRA_HIGHLIGHT_STEPS.map(native),
            midtone: ZEBRA_MIDTONE_STEPS.map(native),
        }
    }

    #[cfg(not(opc_core_linked))]
    fn zebra_steps(&self) -> ZebraSteps {
        let _ = (ZEBRA_HIGHLIGHT_STEPS, ZEBRA_MIDTONE_STEPS);
        ZebraSteps::default()
    }

    /// The false-colour key: one swatch per zone, from the core. Empty without it.
    #[cfg(opc_core_linked)]
    fn legend(&self) -> Vec<opc_chrome::LegendBand> {
        let (color_mode, iso) = self.color_axis();
        opc_render::false_color_legend(self.assists.false_color.scale, color_mode, iso)
            .into_iter()
            .map(|band| opc_chrome::LegendBand {
                label: band.label,
                rgb: band.rgb,
            })
            .collect()
    }

    #[cfg(not(opc_core_linked))]
    fn legend(&self) -> Vec<opc_chrome::LegendBand> {
        Vec::new()
    }

    /// Which lattices false colour wants right now, or none while it is off.
    fn false_color_wanted(&self) -> Option<FalseColorKey> {
        let (color_mode, iso) = self.color_axis();
        self.toggles.false_color.then_some(FalseColorKey {
            scale: self.assists.false_color.scale,
            color_mode,
            iso,
        })
    }

    /// Asks the window for new lattices when the scale, colour mode or ISO moved.
    fn sync_false_color(&mut self) {
        let wanted = self.false_color_wanted();
        if wanted != self.false_color_key {
            self.false_color_key = wanted;
            self.lut_pending.push_back(LutRequest::FalseColor(wanted));
        }
    }

    /// A toolbar chip tapped: the tool flips, and whatever it paints follows.
    fn tap_tool(&mut self, tool: AssistTool) -> Vec<Intent> {
        self.chrome_stale = true;
        match tool {
            AssistTool::Lut => return self.act(Action::ToggleGrade, 0.0),
            AssistTool::Peak => return self.act(Action::TogglePeaking, 0.0),
            AssistTool::Zebra => return self.act(Action::ToggleZebra, 0.0),
            AssistTool::Mirror => return self.act(Action::ToggleMirror, 0.0),
            AssistTool::False => {
                self.toggles.false_color = !self.toggles.false_color;
                self.sync_false_color();
            }
            AssistTool::Guides => self.toggles.guides = !self.toggles.guides,
            AssistTool::Grid => self.toggles.grid = !self.toggles.grid,
            AssistTool::Cross => self.toggles.cross = !self.toggles.cross,
            AssistTool::Wave
            | AssistTool::Parade
            | AssistTool::Histo
            | AssistTool::Vector
            | AssistTool::Lights
            | AssistTool::Nd
            | AssistTool::Audio => {}
        }
        self.refresh_assists();
        Vec::new()
    }

    /// The toolbar's chips, in the phones' order.
    fn assist_chips(&self) -> Vec<AssistChip> {
        AssistTool::TOOLBAR
            .iter()
            .map(|tool| AssistChip {
                label: tool.label().to_string(),
                on: self.tool_on(*tool),
                available: tool.available(),
                group: tool.group(),
            })
            .collect()
    }

    /// What is drawn over the picture, at the fitted rectangle.
    fn overlays(&self) -> Overlays {
        let fit = self.hud.fit.unwrap_or(opc_ui::Fit {
            x: 0.0,
            y: 0.0,
            width: f64::from(self.window.0),
            height: f64::from(self.window.1),
        });
        let feed = (
            fit.x as f32,
            fit.y as f32,
            fit.width as f32,
            fit.height as f32,
        );
        let grid = self.assists.grid;
        let guides = self.assists.guides;
        Overlays {
            grid_thirds: self.toggles.grid && grid.thirds,
            grid_phi: self.toggles.grid && grid.phi,
            grid_diagonal: self.toggles.grid && grid.diagonal,
            guides: if self.toggles.guides {
                guides
                    .frames()
                    .into_iter()
                    .map(|aspect| guide_rect(feed, aspect.ratio()))
                    .collect()
            } else {
                Vec::new()
            },
            guide_mask: guides.mask,
            crosshair: self.toggles.cross,
            legend: if self.toggles.false_color && self.assists.false_color.reference {
                self.legend()
            } else {
                Vec::new()
            },
        }
    }

    /// The names the LUT row offers, found by the window.
    pub fn set_lut_menu(&mut self, menu: LutMenu) {
        self.lut_menu = menu;
        self.chrome_stale = true;
    }

    pub fn lut_choice(&self) -> &LutChoice {
        &self.lut_choice
    }

    /// Any chip, as the sheet would pick it. For tests that do not want to find the
    /// chip by coordinate.
    pub fn pick_for_test(&mut self, pick: Pick) -> Vec<Intent> {
        self.apply_pick(pick)
    }

    /// The ramp setting, as the sheet would set it. For tests that do not want to
    /// find the chip by coordinate.
    pub fn set_ramp_for_test(&mut self, ramp: u8) {
        self.apply_pick(Pick::Ramp(ramp));
    }

    pub fn set_point_for_test(&mut self, slot: Slot) {
        self.apply_pick(Pick::SetPoint(slot));
    }

    pub fn set_leg_for_test(&mut self, slot: Slot, seconds: f64) {
        self.apply_pick(Pick::LegDuration(slot, seconds));
    }

    pub fn start_move_for_test(&mut self) -> Vec<Intent> {
        self.apply_pick(Pick::MoveStart)
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
        // A new attitude push is what makes a pose fresh enough to dispatch against.
        if status.gimbal_attitude_seq != self.attitude_seq {
            self.attitude_seq = status.gimbal_attitude_seq;
            self.attitude_at = self.last_now;
        }
        self.hud.status = status;
        self.chrome_stale = true;
        // A colour-mode or ISO change moves the false-colour zones.
        self.sync_false_color();
    }

    /// The body's live pose, when it has reported one.
    pub fn live_pose(&self) -> Option<Waypoint> {
        Waypoint::from_status(&self.hud.status)
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn move_running(&self) -> bool {
        self.move_engine
            .as_ref()
            .is_some_and(MoveEngine::is_running)
            || self.move_countdown.is_some()
    }

    /// What the top bar says about a take, or nothing.
    fn move_text(&self, now: f64) -> String {
        if let Some(countdown) = self.move_countdown {
            return format!("MOVE · STARTING IN {}", countdown.remaining(now));
        }
        self.move_engine
            .as_ref()
            .map_or_else(String::new, MoveEngine::readout)
    }

    /// Start pressed on the moves sheet: a 3-2-1 like the phones, then the approach.
    fn move_start(&mut self, now: f64) -> Vec<Intent> {
        if self.program.a.is_none() || self.program.b.is_none() || self.live_pose().is_none() {
            return Vec::new();
        }
        self.move_engine = None;
        self.move_countdown = Some(Countdown::start(now, 3.0));
        self.close_sheet();
        Vec::new()
    }

    fn move_stop(&mut self) -> Vec<Intent> {
        self.move_countdown = None;
        let mut intents = Vec::new();
        if let Some(engine) = self.move_engine.as_mut() {
            if engine.is_running() {
                engine.cancel();
                intents.push(Intent::Send(Command::GimbalTimedStop));
            }
        }
        self.chrome_stale = true;
        intents
    }

    /// One frame of a running take.
    fn move_tick(&mut self, now: f64) -> Vec<Intent> {
        let mut intents = Vec::new();
        if let Some(countdown) = self.move_countdown {
            if countdown.remaining(now) != (countdown.remaining(now - 0.05)) {
                self.chrome_stale = true;
            }
            if countdown.is_done(now) {
                self.move_countdown = None;
                let Some(live) = self.live_pose() else {
                    self.chrome_stale = true;
                    return intents;
                };
                match MoveEngine::start(&self.program, live) {
                    Ok(engine) => {
                        self.move_engine = Some(engine);
                        self.move_ticked_at = now;
                    }
                    Err(reason) => {
                        self.hud.phase = self.hud.phase.clone();
                        self.library.status = reason;
                        self.chrome_stale = true;
                        return intents;
                    }
                }
            } else {
                return intents;
            }
        }
        let Some(engine) = self.move_engine.as_mut() else {
            return intents;
        };
        if !engine.is_running() {
            return intents;
        }
        let dt = now - self.move_ticked_at;
        if dt < 0.02 {
            return intents;
        }
        self.move_ticked_at = now;
        let live = Waypoint::from_status(&self.hud.status);
        let age = now - self.attitude_at;
        let output = engine.tick(dt.min(0.12), live, age.max(0.0));
        if let Some((target, duration)) = output.target {
            intents.push(Intent::Send(target.timed_target(duration)));
        }
        if output.stop {
            intents.push(Intent::Send(Command::GimbalTimedStop));
        }
        self.chrome_stale = true;
        intents
    }

    pub fn set_window(&mut self, width: u32, height: u32) {
        if self.window != (width, height) {
            self.window = (width, height);
            self.chrome_stale = true;
            self.chrome = None;
            // Slint hit-tests against its own window size, so a tap before the first
            // frame has drawn must still land on the right control.
            if let Some(cr) = self.chrome_renderer.as_mut() {
                cr.resize(width, height);
            }
        }
    }

    /// The body's model id, when the link knows it. Colour modes encode per model.
    pub fn set_model(&mut self, model_id: Option<i32>) {
        self.model_id = model_id.unwrap_or(-1);
    }

    /// Which sheet is open, if any.
    pub fn sheet(&self) -> Option<SheetKind> {
        self.sheet
    }

    /// Opens a sheet, or closes it when it is the one already open.
    pub fn toggle_sheet(&mut self, kind: SheetKind) {
        self.sheet = if self.sheet == Some(kind) {
            None
        } else {
            Some(kind)
        };
        self.chrome_stale = true;
    }

    pub fn close_sheet(&mut self) {
        if self.sheet.take().is_some() {
            self.chrome_stale = true;
        }
    }

    /// The desktop-side settings, as the sheets show them.
    pub fn prefs(&self) -> Prefs {
        self.prefs
    }

    fn sheet_context(&self) -> sheets::Context<'_> {
        sheets::Context {
            status: &self.hud.status,
            prefs: self.prefs,
            toggles: self.toggles,
            gimbal_mode: self.gimbal_mode,
            model_id: self.model_id,
            luts: &self.lut_menu,
            lut_choice: &self.lut_choice,
            program: &self.program,
            live_pose: self.live_pose(),
            move_running: self.move_running(),
            assists: self.assists,
            zebra_steps: self.zebra_steps(),
        }
    }

    /// Carries out a chip tap on the open sheet.
    fn apply_pick(&mut self, pick: Pick) -> Vec<Intent> {
        self.chrome_stale = true;
        match pick {
            Pick::Send(commands) => commands.into_iter().map(Intent::Send).collect(),
            Pick::Zebra => self.act(Action::ToggleZebra, 0.0),
            Pick::Peaking => self.act(Action::TogglePeaking, 0.0),
            Pick::Grade => self.act(Action::ToggleGrade, 0.0),
            Pick::Mirror => self.act(Action::ToggleMirror, 0.0),
            Pick::Grid(on) => {
                self.toggles.grid = on;
                self.refresh_assists();
                Vec::new()
            }
            Pick::Assist(tool) => self.tap_tool(tool),
            Pick::FalseColorScale(scale) => {
                self.assists.false_color.scale = scale;
                self.sync_false_color();
                Vec::new()
            }
            Pick::FalseColorReference(on) => {
                self.assists.false_color.reference = on;
                Vec::new()
            }
            Pick::PeakingColor(color) => {
                self.assists.peaking_color = color;
                Vec::new()
            }
            Pick::PeakingSense(sense) => {
                self.assists.peaking_sense = sense;
                Vec::new()
            }
            Pick::ZebraUnits(ire) => {
                self.assists.zebra.ire_units = ire;
                Vec::new()
            }
            Pick::ZebraHighlightOn(on) => {
                self.assists.zebra.highlight_on = on;
                Vec::new()
            }
            Pick::ZebraHighlightIre(ire) => {
                self.assists.zebra.highlight_ire = ire;
                Vec::new()
            }
            Pick::ZebraHighlightColor(paint) => {
                self.assists.zebra.highlight_color = paint;
                Vec::new()
            }
            Pick::ZebraMidtoneOn(on) => {
                self.assists.zebra.midtone_on = on;
                Vec::new()
            }
            Pick::ZebraMidtoneIre(ire) => {
                self.assists.zebra.midtone_ire = ire;
                Vec::new()
            }
            Pick::ZebraMidtoneColor(paint) => {
                self.assists.zebra.midtone_color = paint;
                Vec::new()
            }
            Pick::GridLine(line, on) => {
                self.assists.grid.set(line, on);
                Vec::new()
            }
            Pick::GuideFamily(family) => {
                self.assists.guides.family = family;
                Vec::new()
            }
            Pick::GuideAspect(aspect) => {
                self.assists.guides.toggle(aspect);
                Vec::new()
            }
            Pick::GuideMask(on) => {
                self.assists.guides.mask = on;
                Vec::new()
            }
            Pick::Timecode(on) => {
                self.prefs.timecode = on;
                Vec::new()
            }
            Pick::GimbalMode(mode) => {
                self.gimbal_mode = mode;
                mode.commands().into_iter().map(Intent::Send).collect()
            }
            Pick::AudioChannel(channel) => {
                self.prefs.audio_channel = channel;
                vec![Intent::Send(Command::SetAudioChannel(channel))]
            }
            Pick::VocalBoost(boost) => {
                self.prefs.vocal_boost = boost;
                vec![Intent::Send(Command::SetVocalBoost(boost))]
            }
            Pick::Fov(fov) => {
                self.prefs.fov = fov;
                vec![Intent::Send(Command::SetFov(fov))]
            }
            Pick::GimbalSpeed(speed) => {
                self.prefs.gimbal_speed = speed;
                vec![Intent::Send(Command::GimbalSpeed(speed))]
            }
            Pick::Ramp(ramp) => {
                self.prefs.ramp = ramp;
                self.ramp.reset();
                Vec::new()
            }
            Pick::Countdown(seconds) => {
                self.prefs.countdown_seconds = seconds;
                Vec::new()
            }
            Pick::Lut(choice) => {
                self.toggles.grade = choice != LutChoice::Off;
                self.refresh_assists();
                self.lut_choice = choice.clone();
                self.lut_pending.push_back(LutRequest::Load(choice));
                self.refresh_assists();
                Vec::new()
            }
            Pick::SetPoint(slot) => {
                let pose = self.live_pose().filter(Waypoint::is_reachable);
                match slot {
                    Slot::A => self.program.a = pose,
                    Slot::B => self.program.b = pose,
                    Slot::C => self.program.c = pose,
                }
                Vec::new()
            }
            Pick::ClearPoint(slot) => {
                match slot {
                    Slot::A => self.program.a = None,
                    Slot::B => self.program.b = None,
                    Slot::C => self.program.c = None,
                }
                Vec::new()
            }
            Pick::LegDuration(slot, seconds) => {
                match slot {
                    Slot::A => self.program.duration_ab = seconds,
                    _ => self.program.duration_bc = seconds,
                }
                Vec::new()
            }
            Pick::MoveStart => {
                let now = self.last_now;
                self.move_start(now)
            }
            Pick::MoveStop => self.move_stop(),
            Pick::Nothing => Vec::new(),
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
        self.last_now = self.last_now.max(now);
        if key == Key::Escape && self.sheet.is_some() {
            self.close_sheet();
            return Vec::new();
        }
        if self.screen != Screen::Viewfinder {
            return self.press_on_screen(key);
        }
        if key == Key::Char('g') || key == Key::Char('G') {
            return self.open_library();
        }
        if self.stick.set(key, true) {
            // Manual control cancels the path, as on the phones.
            let mut intents = self.move_stop();
            intents.extend(self.stick_changed(now));
            return intents;
        }
        let Some(action) = self.controls.press(key) else {
            return Vec::new();
        };
        self.act(action, now)
    }

    /// A key came up. Only the stick cares.
    pub fn release(&mut self, key: Key, now: f64) -> Vec<Intent> {
        if self.stick.set(key, false) {
            return self.stick_changed(now);
        }
        Vec::new()
    }

    /// The keys' throw changed. Without a ramp the new throw goes out at once; with
    /// one, the filter starts moving toward it and `tick` sends each step.
    fn stick_changed(&mut self, now: f64) -> Vec<Intent> {
        if self.prefs.ramp_tau() <= 0.0 {
            self.stick_sent_at = now;
            return vec![Intent::Send(self.stick.command())];
        }
        // One nominal frame of ease, so the first send is already a fraction of the
        // throw rather than the whole of it.
        self.ramp_ticked_at = self.ramp_ticked_at.max(now - 0.04);
        self.ramp_step(now)
    }

    /// The throw the operator is asking for right now, keys or pad.
    fn stick_target(&self) -> (f64, f64) {
        if self.pad_held {
            self.pad_target
        } else {
            self.stick.target()
        }
    }

    /// One step of the ramp toward the target; sends when the throw moved.
    fn ramp_step(&mut self, now: f64) -> Vec<Intent> {
        let tau = self.prefs.ramp_tau();
        let dt = (now - self.ramp_ticked_at).clamp(0.0, 0.2);
        self.ramp_ticked_at = now;
        let (tx, ty) = self.stick_target();
        let before = self.ramp;
        let (mut x, mut y) = self.ramp.tick(tx, ty, tau, dt);
        if tx == 0.0 && ty == 0.0 && self.ramp.is_settled_at_rest() {
            self.ramp.reset();
            x = 0.0;
            y = 0.0;
        } else if (x - tx).abs() < 0.01 && (y - ty).abs() < 0.01 {
            // Close enough: land on the throw itself, so a held key reaches full.
            self.ramp.x = tx;
            self.ramp.y = ty;
            x = tx;
            y = ty;
        }
        let moved = (x - before.x).abs() > 0.005 || (y - before.y).abs() > 0.005;
        let rested_now = x == 0.0 && y == 0.0 && (before.x != 0.0 || before.y != 0.0);
        if moved || rested_now {
            self.stick_sent_at = now;
            return vec![Intent::Send(opc_ui::stick_command(x, y))];
        }
        Vec::new()
    }

    /// Whether the ramp still has somewhere to go.
    fn ramp_busy(&self) -> bool {
        let (tx, ty) = self.stick_target();
        (self.ramp.x - tx).abs() > 0.005 || (self.ramp.y - ty).abs() > 0.005
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
                    None => Some(Countdown::start(
                        now,
                        f64::from(self.prefs.countdown_seconds),
                    )),
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
                self.lut_pending
                    .push_back(LutRequest::Toggle(self.toggles.grade));
                self.refresh_assists();
                Vec::new()
            }
            Action::ToggleChrome => {
                self.chrome_visible = !self.chrome_visible;
                Vec::new()
            }
            Action::ToggleSettings => {
                self.toggle_sheet(SheetKind::Settings);
                Vec::new()
            }
            Action::ToggleExposure => {
                self.toggle_sheet(SheetKind::Exposure);
                Vec::new()
            }
            Action::ToggleMoves => {
                self.toggle_sheet(SheetKind::Moves);
                Vec::new()
            }
            Action::ToggleAssists => {
                self.assist_bar = !self.assist_bar;
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

    /// Maps what the Slint controls fired since the last call onto shell intents. Called
    /// after every pointer event and every render, so a tap answers on the spot rather
    /// than on the next frame.
    fn take_chrome_intents(&mut self) -> Vec<Intent> {
        let mut fired = Vec::new();
        let Some(cr) = self.chrome_renderer.as_ref() else {
            return fired;
        };
        let intents = cr.drain_intents();
        for intent in intents {
            match intent {
                ChromeIntent::RecordToggle => {
                    fired.push(Intent::Send(if self.hud.status.is_recording {
                        Command::RecordStop
                    } else {
                        Command::RecordStart
                    }));
                }
                ChromeIntent::TakeStill => fired.push(Intent::Send(Command::ShootPhoto)),
                ChromeIntent::GimbalFlip => fired.push(Intent::Send(Command::GimbalFlip)),
                ChromeIntent::GimbalRecenter => {
                    fired.push(Intent::Send(Command::GimbalRecenter));
                }
                ChromeIntent::ZoomSet(v) => {
                    let z = (v as f64).clamp(ZOOM_MIN, ZOOM_MAX);
                    self.controls.set_zoom(z);
                    self.chrome_stale = true;
                    fired.push(Intent::Send(Command::ZoomFactor(z)));
                }
                ChromeIntent::GimbalMoved { x, y } => {
                    // The pad reports right and up positive, the same axes as the keys.
                    self.pad_held = true;
                    self.pad_target = (f64::from(x), f64::from(y));
                    if self.prefs.ramp_tau() > 0.0 {
                        // A pointer event carries no clock: step one nominal frame.
                        let now = self.ramp_ticked_at + 0.04;
                        fired.extend(self.ramp_step(now));
                    } else {
                        fired.push(Intent::Send(opc_ui::stick_command(
                            f64::from(x),
                            f64::from(y),
                        )));
                    }
                }
                ChromeIntent::GimbalReleased => {
                    self.pad_held = false;
                    self.pad_target = (0.0, 0.0);
                    if self.prefs.ramp_tau() > 0.0 && self.stick.is_resting() {
                        // The ramp eases the stick back; the ticks send the steps.
                        let now = self.ramp_ticked_at + 0.04;
                        fired.extend(self.ramp_step(now));
                    } else {
                        fired.push(Intent::Send(Command::GimbalStick {
                            axis0: 1024,
                            axis1: 1024,
                        }));
                    }
                }
                ChromeIntent::FollowToggle => {
                    // ON is Follow; OFF is the tilt-locked follow the body offers.
                    self.gimbal_mode = if self.gimbal_mode == GimbalMode::Follow {
                        GimbalMode::TiltLocked
                    } else {
                        GimbalMode::Follow
                    };
                    self.chrome_stale = true;
                    fired.extend(self.gimbal_mode.commands().into_iter().map(Intent::Send));
                }
                ChromeIntent::FollowCycle => {
                    self.gimbal_mode = self.gimbal_mode.next();
                    self.chrome_stale = true;
                    fired.extend(self.gimbal_mode.commands().into_iter().map(Intent::Send));
                }
                ChromeIntent::ModeSelected(index) => {
                    if let Some(code) = MODE_CODES.get(index).copied().flatten() {
                        fired.push(Intent::Send(Command::SetShootingMode(code)));
                    }
                }
                ChromeIntent::OpenFormat => self.toggle_sheet(SheetKind::Format),
                ChromeIntent::OpenExposure => self.toggle_sheet(SheetKind::Exposure),
                ChromeIntent::OpenMenu => self.toggle_sheet(SheetKind::Settings),
                ChromeIntent::AssistBarToggle => {
                    self.assist_bar = !self.assist_bar;
                    self.chrome_stale = true;
                }
                ChromeIntent::AssistTap(index) => {
                    if let Some(tool) = AssistTool::TOOLBAR.get(index) {
                        fired.extend(self.tap_tool(*tool));
                    }
                }
                ChromeIntent::AssistConfigure(index) => {
                    if let Some(tool) = AssistTool::TOOLBAR.get(index) {
                        self.toggle_sheet(SheetKind::Assist(*tool));
                    }
                }
                ChromeIntent::SheetClose => self.close_sheet(),
                ChromeIntent::SheetTab(tab) => {
                    self.sheet_tab = tab;
                    self.chrome_stale = true;
                }
                ChromeIntent::SheetPick { row, option } => {
                    let pick = self.sheet.and_then(|kind| {
                        sheets::build(kind, self.sheet_tab, self.sheet_context())
                            .pick(row, option)
                            .cloned()
                    });
                    if let Some(pick) = pick {
                        fired.extend(self.apply_pick(pick));
                    }
                }
                ChromeIntent::Exit => fired.push(Intent::Quit),
                ChromeIntent::OpenGallery => fired.extend(self.open_library()),
                ChromeIntent::LibraryBack => fired.extend(self.close_library()),
                ChromeIntent::LibraryTab(index) => {
                    if let Some(tab) = opc_media::LibraryTab::ALL.get(index) {
                        self.library.tab = *tab;
                        self.chrome_stale = true;
                    }
                }
                ChromeIntent::LibrarySortNext => {
                    self.library.sort = self.library.sort.next();
                    self.chrome_stale = true;
                }
                ChromeIntent::LibraryRefresh => {
                    fired.extend(self.press_on_screen(Key::Char('r')));
                }
                ChromeIntent::LibrarySelect(index) => {
                    self.library.select_index(index);
                    self.chrome_stale = true;
                }
                ChromeIntent::LibraryPlay => fired.extend(self.library_open_selected()),
                ChromeIntent::LibraryDownload => {
                    if let Some(file) = self.library.selected_file().cloned() {
                        self.library.progress.insert(file.path.clone(), (0, None));
                        self.chrome_stale = true;
                        fired.push(Intent::Media(MediaAction::Download(file)));
                    }
                }
                ChromeIntent::LibraryFavorite => {
                    let counter = self.next_media_counter();
                    if let Some(command) = self.library.toggle_favorite(counter) {
                        fired.push(Intent::Send(command));
                    }
                    self.chrome_stale = true;
                }
                ChromeIntent::LibraryDelete => {
                    let counter = self.next_media_counter();
                    if let Some(command) = self.library.delete_tapped(counter) {
                        fired.push(Intent::Send(command));
                    }
                    self.chrome_stale = true;
                }
                ChromeIntent::LibrarySource(local) => {
                    self.library.local = local;
                    self.library.selected = None;
                    self.chrome_stale = true;
                }
                ChromeIntent::PlayerBack => fired.extend(self.close_player()),
                ChromeIntent::PlayerToggle => fired.extend(self.player_toggle()),
                ChromeIntent::PlayerInfo => {
                    if let Some(player) = self.player.as_mut() {
                        player.show_info = !player.show_info;
                        self.chrome_stale = true;
                    }
                }
                ChromeIntent::PlayerDownload => {
                    if let Some(file) = self.player.as_ref().map(|p| p.file.clone()) {
                        self.library.progress.insert(file.path.clone(), (0, None));
                        fired.push(Intent::Media(MediaAction::Download(file)));
                    }
                }
                ChromeIntent::PlayerScreenshot => fired.push(Intent::Still),
                ChromeIntent::PlayerLut => fired.extend(self.act(Action::ToggleGrade, 0.0)),
                ChromeIntent::PlayerZebra => fired.extend(self.act(Action::ToggleZebra, 0.0)),
                ChromeIntent::PlayerPeaking => {
                    fired.extend(self.act(Action::TogglePeaking, 0.0));
                }
                ChromeIntent::PlayerFavorite => {
                    if self.player_select() {
                        let counter = self.next_media_counter();
                        if let Some(command) = self.library.toggle_favorite(counter) {
                            fired.push(Intent::Send(command));
                        }
                        self.chrome_stale = true;
                    }
                }
                ChromeIntent::PlayerDelete => {
                    if self.player_select() {
                        let counter = self.next_media_counter();
                        if let Some(command) = self.library.delete_tapped(counter) {
                            fired.push(Intent::Send(command));
                            // The clip is gone: back to the grid.
                            fired.extend(self.close_player());
                        }
                        self.chrome_stale = true;
                    }
                }
                ChromeIntent::PlayerSeek(fraction) => {
                    if let Some(player) = self.player.as_mut() {
                        let position = (f64::from(fraction).clamp(0.0, 1.0)
                            * player.duration_ms as f64)
                            as i64;
                        player.position_ms = position;
                        self.chrome_stale = true;
                        fired.push(Intent::Media(MediaAction::PlayerSeek(position)));
                    }
                }
                ChromeIntent::FullscreenToggle => fired.push(Intent::ToggleFullscreen),
                // Surfaces that do not exist on the desktop yet.
                ChromeIntent::OrientationToggle => {}
            }
        }
        fired
    }

    // ── Screens ──────────────────────────────────────────────────────────────

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    pub fn library_mut(&mut self) -> &mut Library {
        self.chrome_stale = true;
        &mut self.library
    }

    pub fn player(&self) -> Option<&Player> {
        self.player.as_ref()
    }

    fn next_media_counter(&mut self) -> u32 {
        self.media_counter = self.media_counter.wrapping_add(1);
        self.media_counter
    }

    /// The gallery button, or `G`: the library comes up over the picture.
    pub fn open_library(&mut self) -> Vec<Intent> {
        if self.screen == Screen::Library {
            return Vec::new();
        }
        self.close_sheet();
        self.screen = Screen::Library;
        self.player = None;
        self.library.listing = true;
        self.library.status.clear();
        self.chrome_stale = true;
        vec![Intent::Media(MediaAction::OpenLibrary)]
    }

    pub fn close_library(&mut self) -> Vec<Intent> {
        if self.screen == Screen::Viewfinder {
            return Vec::new();
        }
        self.screen = Screen::Viewfinder;
        self.player = None;
        self.library.delete_armed = None;
        self.chrome_stale = true;
        vec![Intent::Media(MediaAction::CloseLibrary)]
    }

    /// The window listed a page: append what is new.
    pub fn library_listed(&mut self, files: Vec<MediaFile>, done: bool) {
        for file in files {
            if !self
                .library
                .files
                .iter()
                .any(|known| known.path == file.path)
            {
                self.library.files.push(file);
            }
        }
        self.library.listing = !done;
        self.chrome_stale = true;
    }

    pub fn library_status(&mut self, status: impl Into<String>) {
        self.library.status = status.into();
        self.library.listing = false;
        self.chrome_stale = true;
    }

    /// A thumbnail decoded: the chrome keeps it by path.
    pub fn library_thumb(&mut self, path: &str, width: u32, height: u32, rgba: &[u8]) {
        if let Some(cr) = self.chrome_renderer.as_mut() {
            cr.set_thumb(path, width, height, rgba);
        }
        self.chrome_stale = true;
    }

    pub fn library_progress(&mut self, path: &str, done: u64, total: Option<u64>) {
        self.library
            .progress
            .insert(path.to_string(), (done, total));
        self.chrome_stale = true;
    }

    /// A file landed on disk.
    pub fn library_file_ready(&mut self, path: &str, proxy: bool) {
        self.library.progress.remove(path);
        if proxy {
            self.library.proxies.insert(path.to_string());
        } else {
            self.library.cached.insert(path.to_string());
            self.library
                .notes
                .insert(path.to_string(), "Saved to the library folder".to_string());
        }
        self.chrome_stale = true;
    }

    pub fn library_failed(&mut self, path: &str, reason: &str) {
        self.library.progress.remove(path);
        self.library
            .notes
            .insert(path.to_string(), format!("Could not fetch: {reason}"));
        self.chrome_stale = true;
    }

    /// The window opened a clip in the player, or a still in the viewer.
    pub fn open_player(&mut self, file: MediaFile, duration_ms: i64, proxy: bool, is_photo: bool) {
        self.screen = if is_photo {
            Screen::Photo
        } else {
            Screen::Player
        };
        self.library.delete_armed = None;
        self.player = Some(Player {
            file,
            playing: !is_photo,
            position_ms: 0,
            duration_ms,
            proxy,
            is_photo,
            show_info: false,
        });
        self.chrome_stale = true;
    }

    /// Frames across the clip for the filmstrip scrubber.
    pub fn player_strip(&mut self, path: &str, frames: &[(u32, u32, Vec<u8>)]) {
        if let Some(cr) = self.chrome_renderer.as_mut() {
            cr.set_strip(path, frames);
        }
        self.chrome_stale = true;
    }

    /// The player's heart or trash: the clip on screen becomes the selection, so the
    /// library's own rules apply.
    fn player_select(&mut self) -> bool {
        let Some(path) = self.player.as_ref().map(|player| player.file.path.clone()) else {
            return false;
        };
        self.library.selected = Some(path);
        true
    }

    pub fn player_position(&mut self, position_ms: i64) {
        if let Some(player) = self.player.as_mut() {
            if (player.position_ms / 1000) != (position_ms / 1000) {
                self.chrome_stale = true;
            }
            player.position_ms = position_ms;
        }
    }

    /// The clip ran out: the transport shows the end, paused.
    pub fn player_ended(&mut self) {
        if let Some(player) = self.player.as_mut() {
            player.playing = false;
            player.position_ms = player.duration_ms;
            self.chrome_stale = true;
        }
    }

    fn press_on_screen(&mut self, key: Key) -> Vec<Intent> {
        match (self.screen, key) {
            (Screen::Library, Key::Escape) => self.close_library(),
            (Screen::Library, Key::Char('r' | 'R')) => {
                self.library.listing = true;
                self.library.status.clear();
                self.chrome_stale = true;
                vec![Intent::Media(MediaAction::Refresh)]
            }
            (Screen::Player | Screen::Photo, Key::Escape) => self.close_player(),
            (Screen::Player, Key::Space) => self.player_toggle(),
            (_, Key::Char('h' | 'H')) => {
                self.chrome_visible = !self.chrome_visible;
                self.chrome_stale = true;
                Vec::new()
            }
            (_, Key::Char('z' | 'Z')) => self.act(Action::ToggleZebra, 0.0),
            (_, Key::Char('p' | 'P')) => self.act(Action::TogglePeaking, 0.0),
            (_, Key::Char('l' | 'L')) => self.act(Action::ToggleGrade, 0.0),
            (_, Key::Char('m' | 'M')) => self.act(Action::ToggleMirror, 0.0),
            _ => Vec::new(),
        }
    }

    fn close_player(&mut self) -> Vec<Intent> {
        self.player = None;
        self.screen = Screen::Library;
        self.chrome_stale = true;
        vec![Intent::Media(MediaAction::ClosePlayer)]
    }

    fn player_toggle(&mut self) -> Vec<Intent> {
        if let Some(player) = self.player.as_mut() {
            player.playing = !player.playing;
            self.chrome_stale = true;
            return vec![Intent::Media(MediaAction::PlayerToggle)];
        }
        Vec::new()
    }

    fn library_open_selected(&mut self) -> Vec<Intent> {
        let Some(file) = self.library.selected_file().cloned() else {
            return Vec::new();
        };
        self.chrome_stale = true;
        if file.is_video() {
            vec![Intent::Media(MediaAction::Play(file))]
        } else {
            vec![Intent::Media(MediaAction::Photo(file))]
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
        // An open sheet owns the window: the scrim around it is a close button. So
        // does any screen but the viewfinder: there is no picture to draw a box on.
        if self.sheet.is_some() || self.screen != Screen::Viewfinder {
            return true;
        }
        if let Some(cr) = &self.chrome_renderer {
            cr.is_over_control(x, y, self.window.0, self.window.1)
        } else {
            false
        }
    }

    /// Pointer pressed in a control zone: forward to Slint.
    /// Returns `Some([])` so the caller knows it was claimed (even if no intent fired yet).
    pub fn control_down(&mut self, x: f64, y: f64, now: f64) -> Option<Vec<Intent>> {
        self.last_now = self.last_now.max(now);
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
        Some(self.take_chrome_intents())
    }

    /// Pointer moved while a Slint control is held.
    pub fn control_moved(&mut self, x: f64, y: f64) -> Vec<Intent> {
        if let Some(cr) = &self.chrome_renderer {
            cr.pointer_moved(x as f32, y as f32);
        }
        self.take_chrome_intents()
    }

    /// The window's clock, for the ramp when a pointer event carries none.
    pub fn note_time(&mut self, now: f64) {
        if self.ramp_ticked_at < now
            && !self.ramp_busy()
            && self.ramp == opc_ui::RampFilter::default()
        {
            self.ramp_ticked_at = now;
        }
    }

    /// Pointer released over a control zone.
    pub fn control_up(&mut self, x: f64, y: f64, now: f64) -> Vec<Intent> {
        self.last_now = self.last_now.max(now);
        if let Some(cr) = &self.chrome_renderer {
            cr.pointer_released(x as f32, y as f32);
        }
        self.chrome_stale = true;
        self.take_chrome_intents()
    }

    /// Focus lost or gesture cancelled. Slint drops its pressed state, and a joystick
    /// that was being held rests the camera at once: a stick left thrown by a palm or a
    /// window that lost focus is a gimbal that keeps moving.
    pub fn control_cancel(&mut self) -> Vec<Intent> {
        if let Some(cr) = &self.chrome_renderer {
            cr.pointer_cancel();
        }
        let mut intents = self.take_chrome_intents();
        if self.pad_held {
            self.pad_held = false;
            self.chrome_stale = true;
            intents.push(Intent::Send(Command::GimbalStick {
                axis0: 1024,
                axis1: 1024,
            }));
        }
        intents
    }

    /// The clock moved on: fires the countdown, keeps the stick alive, and
    /// returns any intents fired by Slint controls since last tick.
    pub fn tick(&mut self, now: f64) -> Vec<Intent> {
        self.last_now = self.last_now.max(now);
        let mut intents = Vec::new();
        // Drain intents queued by Slint button callbacks.
        intents.append(&mut self.chrome_pending_intents);
        intents.extend(self.move_tick(now));
        if self.hud.countdown_fired(now) {
            self.hud.countdown = None;
            self.chrome_stale = true;
            intents.push(Intent::Send(Command::RecordStart));
        }
        if self.prefs.ramp_tau() > 0.0 {
            // The ramp steps at 25 Hz while it moves, and keeps a held throw alive.
            let held = self.stick_target() != (0.0, 0.0);
            if (self.ramp_busy()
                || !self.ramp.is_settled_at_rest()
                || self.ramp != opc_ui::RampFilter::default())
                && now - self.ramp_ticked_at >= 0.04
            {
                intents.extend(self.ramp_step(now));
            } else if held && now - self.stick_sent_at >= STICK_REPEAT {
                self.stick_sent_at = now;
                intents.push(Intent::Send(opc_ui::stick_command(
                    self.ramp.x,
                    self.ramp.y,
                )));
            }
        } else if !self.stick.is_resting() && now - self.stick_sent_at >= STICK_REPEAT {
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

            let controls_enabled = self.controls_enabled();
            let sheet = self
                .sheet
                .map(|kind| sheets::build(kind, self.sheet_tab, self.sheet_context()).sheet);
            let timecode = if self.prefs.timecode {
                self.hud.status.timecode.clone().unwrap_or_default()
            } else {
                String::new()
            };
            let overlays = self.overlays();
            let assist_bar = self.assist_bar.then(|| self.assist_chips());
            let move_text = self.move_text(now);
            let screen = self.screen;
            let grid_width = self.window.0 as f32;
            let library = (screen == Screen::Library).then(|| self.library.state(grid_width));
            if screen == Screen::Library {
                for file in self.library.thumbs_wanted() {
                    self.chrome_pending_intents
                        .push(Intent::Media(MediaAction::Thumb(file)));
                }
            }
            let player = self
                .player
                .as_ref()
                .map(|player| player.state(&self.library, self.toggles));
            let mut canvas = if let Some(cr) = self.chrome_renderer.as_mut() {
                let status = &self.hud.status;
                let link_state = self.hud.connection_chip();
                let zoom = self.controls.zoom();
                let battery_pct = status.battery_percent.unwrap_or(0);
                let fit = self.hud.fit.unwrap_or(opc_ui::Fit {
                    x: 0.0,
                    y: 0.0,
                    width: f64::from(self.window.0),
                    height: f64::from(self.window.1),
                });
                let mode = mode_index(status.shooting_mode);
                let state = ChromeState {
                    phase: &self.hud.phase,
                    shutter: status.shutter_label().unwrap_or_default(),
                    iso: status.iso.map(|iso| iso.to_string()).unwrap_or_default(),
                    ev: status.ev_label().unwrap_or_default(),
                    wb: status
                        .white_balance_kelvin
                        .filter(|value| *value > 0)
                        .map(|kelvin| format!("{kelvin}K"))
                        .unwrap_or_default(),
                    link_state,
                    is_recording: status.is_recording,
                    rec_elapsed: status.elapsed_label(),
                    follow_on: self.gimbal_mode == GimbalMode::Follow,
                    format_label: status.format_label(),
                    expo_label: match status.expo_mode {
                        Some(0x04) => "M".to_string(),
                        _ => "AUTO".to_string(),
                    },
                    battery_text: format!("{battery_pct}%"),
                    battery_percent: battery_pct,
                    storage_text: status.remaining_label(),
                    zoom: zoom as f32,
                    zoom_label: format!("{:.1}×", zoom),
                    mode,
                    photo_mode: mode == 4,
                    controls_enabled,
                    fit: (
                        fit.x.max(0.0) as u32,
                        fit.y.max(0.0) as u32,
                        fit.width.max(0.0) as u32,
                        fit.height.max(0.0) as u32,
                    ),
                    countdown: second,
                    fps_shown: self.hud.fps,
                    timecode,
                    overlays,
                    assist_bar,
                    move_text,
                    sheet,
                    screen,
                    library,
                    player,
                };
                cr.render(&state, self.window.0, self.window.1)
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
            // Anything a Slint control fired while its state was pushed.
            let fired = self.take_chrome_intents();
            self.chrome_pending_intents.extend(fired);
        }
        self.chrome.as_ref()
    }

    /// Whether the chrome is being drawn at all.
    pub fn chrome_visible(&self) -> bool {
        self.chrome_visible
    }
}
