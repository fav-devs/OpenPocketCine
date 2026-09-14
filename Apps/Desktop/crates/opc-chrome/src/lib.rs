//! Viewfinder chrome rendered by Slint's software renderer.
//!
//! [`Chrome`] renders the top/bottom bars, zoom slider, and right-panel controls
//! into a transparent RGBA overlay.  The shell composites that buffer over the
//! video frame.  Pointer events forwarded from winit via [`Chrome::pointer_pressed`]
//! etc. drive the interactive Slint controls; intents are pulled back via
//! [`Chrome::drain_intents`] each frame.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::{
    software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType},
    Platform, PlatformError, PointerEventButton, WindowEvent,
};
use std::collections::HashMap;

use slint::{
    Image, LogicalPosition, ModelRc, PhysicalSize, Rgba8Pixel, SharedPixelBuffer, SharedString,
    VecModel,
};

use opc_ui::canvas::Canvas;
use opc_ui::hud::Phase;

// Slint's generated component types carry no `Debug`; the workspace lint would flag them.
mod generated {
    #![allow(missing_debug_implementations)]
    slint::include_modules!();
}
use generated::{HudOverlay, MediaCell, MediaSelection, PlayerView as SlintPlayerView, SheetRow};
use slint::ComponentHandle;

// ── Custom RGBA pixel ────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct RgbaPixel([u8; 4]);

impl slint::platform::software_renderer::TargetPixel for RgbaPixel {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let src_a = u32::from(color.alpha);
        if src_a == 0 {
            return;
        }
        if src_a == 255 {
            self.0 = [color.red, color.green, color.blue, 255];
            return;
        }
        let sr = u32::from(color.red) * 255 / src_a;
        let sg = u32::from(color.green) * 255 / src_a;
        let sb = u32::from(color.blue) * 255 / src_a;

        let dst_a = u32::from(self.0[3]);
        let inv = 255 - src_a;

        if dst_a == 0 {
            self.0 = [sr as u8, sg as u8, sb as u8, src_a as u8];
        } else {
            let out_a = (src_a + dst_a * inv / 255).min(255);
            let blend_ch = |s: u32, d: u8| {
                ((s * src_a + u32::from(d) * dst_a * inv / 255) / out_a).min(255) as u8
            };
            self.0 = [
                blend_ch(sr, self.0[0]),
                blend_ch(sg, self.0[1]),
                blend_ch(sb, self.0[2]),
                out_a as u8,
            ];
        }
    }

    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        RgbaPixel([r, g, b, 255])
    }

    /// The overlay composites over the video frame, so the clear colour must be
    /// fully transparent. The trait default is opaque black, which would hide the
    /// picture under the chrome.
    fn background() -> Self {
        RgbaPixel([0, 0, 0, 0])
    }
}

// ── Platform ─────────────────────────────────────────────────────────────────

struct OpcPlatform {
    window: Rc<MinimalSoftwareWindow>,
    started: Instant,
}

impl Platform for OpcPlatform {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.started.elapsed()
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Other(
            "opc-chrome drives its own event loop".into(),
        ))
    }
}

thread_local! {
    static SW_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

fn ensure_platform(started: Instant) -> Rc<MinimalSoftwareWindow> {
    SW_WINDOW.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
            *slot = Some(window.clone());
            let _ = slint::platform::set_platform(Box::new(OpcPlatform { window, started }));
        }
        slot.as_ref().unwrap().clone()
    })
}

// ── Intents fired by Slint controls ─────────────────────────────────────────

/// An action triggered by a Slint control (button tap, slider drag, gimbal).
#[derive(Debug, Clone)]
pub enum ChromeIntent {
    RecordToggle,
    TakeStill,
    GimbalFlip,
    GimbalRecenter,
    /// Slider value in the range 1.0 – 6.0.
    ZoomSet(f32),
    /// Normalised stick deflection in -1.0 … 1.0 on each axis.
    GimbalMoved {
        x: f32,
        y: f32,
    },
    /// Stick released — camera should return to centre.
    GimbalReleased,
    /// The `⋮` in the top bar: open the settings panel.
    OpenMenu,
    /// Gimbal follow toggle in the top bar.
    FollowToggle,
    /// Cycle the gimbal follow mode (Follow / Tilt locked / FPV).
    FollowCycle,
    /// Resolution / frame-rate chip.
    OpenFormat,
    /// Exposure chip (AUTO / M).
    OpenExposure,
    /// Leave the viewfinder.
    Exit,
    /// Media gallery.
    OpenGallery,
    /// Landscape / portrait switch.
    OrientationToggle,
    /// Fullscreen window toggle.
    FullscreenToggle,
    /// A mode tapped in the strip, by index into [`MODES`].
    ModeSelected(usize),
    /// A chip tapped in the open sheet: which row, which option.
    SheetPick {
        row: usize,
        option: usize,
    },
    /// A tab tapped in the open sheet.
    SheetTab(usize),
    /// The sheet's close button, or a tap on the scrim around it.
    SheetClose,
    // The library.
    LibraryBack,
    LibraryTab(usize),
    LibrarySortNext,
    LibraryRefresh,
    /// A cell tapped, by index into the cells the shell passed.
    LibrarySelect(usize),
    LibraryPlay,
    LibraryDownload,
    LibraryFavorite,
    LibraryDelete,
    // The player.
    PlayerBack,
    PlayerToggle,
    /// Where on the clip to go, 0…1.
    PlayerSeek(f32),
}

/// Which screen the chrome draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Viewfinder,
    Library,
    Player,
    Photo,
}

impl Screen {
    fn index(self) -> i32 {
        match self {
            Self::Viewfinder => 0,
            Self::Library => 1,
            Self::Player => 2,
            Self::Photo => 3,
        }
    }
}

/// One tile of the library grid. The thumbnail itself is looked up by path in the
/// chrome's own cache, filled by [`Chrome::set_thumb`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellState {
    pub path: String,
    pub title: String,
    /// The duration badge for a clip, or the kind for a still.
    pub meta: String,
    pub is_video: bool,
    pub starred: bool,
    pub cached: bool,
    pub selected: bool,
}

/// The selected file's bar at the bottom of the library.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionState {
    pub title: String,
    pub meta: String,
    pub is_video: bool,
    pub starred: bool,
    pub cached: bool,
    pub deletable: bool,
    pub delete_armed: bool,
    /// A transfer in flight, 0…1.
    pub progress: Option<f32>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryState {
    pub tab: usize,
    pub sort_label: String,
    pub status: String,
    pub cells: Vec<CellState>,
    pub selection: Option<SelectionState>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub title: String,
    pub tag: String,
    pub position_label: String,
    pub duration_label: String,
    pub progress: f32,
    pub playing: bool,
    pub assists: String,
    pub is_photo: bool,
}

/// One row of a sheet: a title and the chips beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetRowState {
    pub title: String,
    pub options: Vec<String>,
    /// Which chip is lit, if any.
    pub selected: Option<usize>,
    /// A row the operator cannot use right now is drawn but greyed.
    pub enabled: bool,
}

/// A sheet over the picture: the format picker, the exposure sheet or the settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SheetState {
    pub title: String,
    pub tabs: Vec<String>,
    pub tab: usize,
    pub rows: Vec<SheetRowState>,
}

/// The mode strip, in Mimo's order. Indices are what [`ChromeIntent::ModeSelected`] carries.
pub const MODES: [&str; 7] = [
    "TIMELAPSE",
    "SLOWMOTION",
    "LOW-LIGHT",
    "VIDEO",
    "PHOTO",
    "PANO",
    "LIVESTREAM",
];

// ── State passed by the shell each frame ─────────────────────────────────────

/// Everything the chrome needs to know for one frame.
#[derive(Debug)]
pub struct ChromeState<'a> {
    pub phase: &'a Phase,
    /// Exposure readouts in the left column. Empty means the camera has not said.
    pub shutter: String,
    pub iso: String,
    pub ev: String,
    pub wb: String,
    pub link_state: &'a str,
    pub is_recording: bool,
    pub rec_elapsed: String,
    /// Top-bar chips.
    pub follow_on: bool,
    /// e.g. "1080P·60".
    pub format_label: String,
    /// "AUTO" or "M".
    pub expo_label: String,
    /// Right-column status.
    pub battery_text: String,
    pub battery_percent: i32,
    pub storage_text: String,
    /// Current zoom (1.0 – 6.0) — drives the dial.
    pub zoom: f32,
    /// Formatted zoom label, e.g. "1.0×".
    pub zoom_label: String,
    /// Index into [`MODES`].
    pub mode: usize,
    /// The record button fires a still instead.
    pub photo_mode: bool,
    /// Controls are greyed while the link is recovering or failed.
    pub controls_enabled: bool,
    /// Where the fitted picture sits in the window, in physical pixels: x, y, w, h.
    pub fit: (u32, u32, u32, u32),
    /// Seconds left on the take countdown, while one runs.
    pub countdown: Option<u32>,
    /// Frames reaching the screen per second; zero hides the readout.
    pub fps_shown: u32,
    /// The body's timecode, when the operator wants it in the top bar.
    pub timecode: String,
    /// Rule-of-thirds lines over the picture.
    pub grid_on: bool,
    /// The open sheet, if any.
    pub sheet: Option<SheetState>,
    pub screen: Screen,
    pub library: Option<LibraryState>,
    pub player: Option<PlayerState>,
}

// Layout metrics mirrored from `hud.slint`; `is_over_control` uses them for hit zones.
const TOP_BAR_H: f64 = 56.0;
const BOTTOM_BAR_H: f64 = 152.0;
const ZOOM_DIAL_W: f64 = 320.0;
const ZOOM_DIAL_H: f64 = 76.0;
const SIDE_COLUMN_W: f64 = 110.0;

// ── Chrome ───────────────────────────────────────────────────────────────────

pub struct Chrome {
    window: Rc<MinimalSoftwareWindow>,
    component: HudOverlay,
    pixels: Vec<RgbaPixel>,
    size: (u32, u32),
    intents: Rc<RefCell<Vec<ChromeIntent>>>,
    /// Thumbnails by camera path, built once and reused across redraws.
    thumbs: HashMap<String, Image>,
}

impl std::fmt::Debug for Chrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chrome").field("size", &self.size).finish()
    }
}

impl Chrome {
    pub fn new(started: Instant) -> Result<Self, String> {
        let window = ensure_platform(started);
        let component = HudOverlay::new().map_err(|e| e.to_string())?;
        component.show().map_err(|e| e.to_string())?;

        let intents: Rc<RefCell<Vec<ChromeIntent>>> = Rc::new(RefCell::new(Vec::new()));

        // Wire Slint callbacks → intent queue.
        {
            let q = intents.clone();
            component.on_record_tapped(move || {
                q.borrow_mut().push(ChromeIntent::RecordToggle);
            });
        }
        {
            let q = intents.clone();
            component.on_still_tapped(move || {
                q.borrow_mut().push(ChromeIntent::TakeStill);
            });
        }
        {
            let q = intents.clone();
            component.on_flip_tapped(move || {
                q.borrow_mut().push(ChromeIntent::GimbalFlip);
            });
        }
        {
            let q = intents.clone();
            component.on_recenter_tapped(move || {
                q.borrow_mut().push(ChromeIntent::GimbalRecenter);
            });
        }
        {
            let q = intents.clone();
            component.on_zoom_changed(move |v| {
                q.borrow_mut().push(ChromeIntent::ZoomSet(v));
            });
        }
        {
            let q = intents.clone();
            component.on_gimbal_moved(move |x, y| {
                q.borrow_mut().push(ChromeIntent::GimbalMoved { x, y });
            });
        }
        {
            let q = intents.clone();
            component.on_gimbal_released(move || {
                q.borrow_mut().push(ChromeIntent::GimbalReleased);
            });
        }
        macro_rules! simple {
            ($setter:ident, $intent:expr) => {{
                let q = intents.clone();
                component.$setter(move || {
                    q.borrow_mut().push($intent);
                });
            }};
        }
        simple!(on_menu_tapped, ChromeIntent::OpenMenu);
        simple!(on_follow_tapped, ChromeIntent::FollowToggle);
        simple!(on_follow_cycle_tapped, ChromeIntent::FollowCycle);
        simple!(on_format_tapped, ChromeIntent::OpenFormat);
        simple!(on_expo_tapped, ChromeIntent::OpenExposure);
        simple!(on_exit_tapped, ChromeIntent::Exit);
        simple!(on_gallery_tapped, ChromeIntent::OpenGallery);
        simple!(on_orientation_tapped, ChromeIntent::OrientationToggle);
        simple!(on_fullscreen_tapped, ChromeIntent::FullscreenToggle);
        {
            let q = intents.clone();
            component.on_mode_selected_changed(move |i| {
                if i >= 0 {
                    q.borrow_mut().push(ChromeIntent::ModeSelected(i as usize));
                }
            });
        }
        {
            let q = intents.clone();
            component.on_sheet_picked(move |row, option| {
                if row >= 0 && option >= 0 {
                    q.borrow_mut().push(ChromeIntent::SheetPick {
                        row: row as usize,
                        option: option as usize,
                    });
                }
            });
        }
        {
            let q = intents.clone();
            component.on_sheet_tab_picked(move |i| {
                if i >= 0 {
                    q.borrow_mut().push(ChromeIntent::SheetTab(i as usize));
                }
            });
        }
        simple!(on_sheet_closed, ChromeIntent::SheetClose);
        simple!(on_library_back, ChromeIntent::LibraryBack);
        simple!(on_library_sort_tapped, ChromeIntent::LibrarySortNext);
        simple!(on_library_refresh, ChromeIntent::LibraryRefresh);
        simple!(on_library_play, ChromeIntent::LibraryPlay);
        simple!(on_library_download, ChromeIntent::LibraryDownload);
        simple!(on_library_favorite, ChromeIntent::LibraryFavorite);
        simple!(on_library_delete, ChromeIntent::LibraryDelete);
        simple!(on_player_back, ChromeIntent::PlayerBack);
        simple!(on_player_toggle, ChromeIntent::PlayerToggle);
        {
            let q = intents.clone();
            component.on_library_tab_picked(move |i| {
                if i >= 0 {
                    q.borrow_mut().push(ChromeIntent::LibraryTab(i as usize));
                }
            });
        }
        {
            let q = intents.clone();
            component.on_library_cell_tapped(move |i| {
                if i >= 0 {
                    q.borrow_mut().push(ChromeIntent::LibrarySelect(i as usize));
                }
            });
        }
        {
            let q = intents.clone();
            component.on_player_seek(move |p| {
                q.borrow_mut().push(ChromeIntent::PlayerSeek(p));
            });
        }

        Ok(Chrome {
            window,
            component,
            pixels: Vec::new(),
            size: (0, 0),
            intents,
            thumbs: HashMap::new(),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self.size != (width, height) {
            self.size = (width, height);
            self.window.set_size(PhysicalSize::new(width, height));
        }
    }

    /// Forward a winit pointer-pressed event to Slint (logical coords).
    pub fn pointer_pressed(&self, x: f32, y: f32) {
        self.window.dispatch_event(WindowEvent::PointerPressed {
            position: LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        });
    }

    /// Forward a winit pointer-released event to Slint.
    pub fn pointer_released(&self, x: f32, y: f32) {
        self.window.dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(x, y),
            button: PointerEventButton::Left,
        });
    }

    /// Forward a winit cursor-moved event to Slint.
    pub fn pointer_moved(&self, x: f32, y: f32) {
        self.window.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(x, y),
        });
    }

    /// The gesture was taken away: the pointer leaves, so pressed controls let go.
    pub fn pointer_cancel(&self) {
        self.window.dispatch_event(WindowEvent::PointerExited);
    }

    /// A thumbnail arrived for a camera path. Tightly packed RGBA.
    pub fn set_thumb(&mut self, path: &str, width: u32, height: u32, rgba: &[u8]) {
        if width == 0 || height == 0 || rgba.len() < (width * height * 4) as usize {
            return;
        }
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            &rgba[..(width * height * 4) as usize],
            width,
            height,
        );
        self.thumbs
            .insert(path.to_string(), Image::from_rgba8(buffer));
    }

    pub fn has_thumb(&self, path: &str) -> bool {
        self.thumbs.contains_key(path)
    }

    /// Drain all ChromeIntents fired since the last call.
    pub fn drain_intents(&self) -> Vec<ChromeIntent> {
        self.intents.borrow_mut().drain(..).collect()
    }

    /// Whether (x, y) in physical pixels falls inside one of the Slint control
    /// zones.  Used by the shell to decide if a pointer down goes to Slint
    /// rather than starting a tracking-box drag.
    pub fn is_over_control(&self, x: f64, y: f64, w: u32, h: u32) -> bool {
        let (w, h) = (w as f64, h as f64);
        // An open sheet owns the whole window: the scrim around it is a close button.
        // So does any screen but the viewfinder: there is no picture to draw a box on.
        if self.component.get_sheet_open() || self.component.get_screen() != 0 {
            return true;
        }
        // Top bar and bottom bar (with the mode strip) hold every button.
        if y <= TOP_BAR_H || y >= h - BOTTOM_BAR_H {
            return true;
        }
        // Zoom dial sits under the top bar, centred.
        let c = &self.component;
        let fit_x = f64::from(c.get_fit_x());
        let fit_w = f64::from(c.get_fit_width());
        let dial_x = fit_x + (fit_w - ZOOM_DIAL_W) / 2.0;
        if y <= TOP_BAR_H + ZOOM_DIAL_H && (dial_x..=dial_x + ZOOM_DIAL_W).contains(&x) {
            return true;
        }
        // Side columns are read-only, but a press there must not start a tracking box.
        if x <= SIDE_COLUMN_W + 16.0 || x >= w - SIDE_COLUMN_W - 16.0 {
            return true;
        }
        false
    }

    pub fn render(&mut self, state: &ChromeState, width: u32, height: u32) -> Canvas {
        self.resize(width, height);
        self.push_state(state);
        slint::platform::update_timers_and_animations();

        let count = (width as usize) * (height as usize);
        if self.pixels.len() != count {
            self.pixels = vec![RgbaPixel::default(); count];
        }

        // ReusedBuffer retains content between frames; only dirty regions are redrawn.
        // Do NOT zero the buffer — that wipes regions Slint didn't mark dirty this frame
        // (buttons, top bar) leaving them transparent until they next change.
        // request_redraw() on size/state changes is enough to mark the whole window dirty.
        self.window.draw_if_needed(|renderer| {
            renderer.render(&mut self.pixels, width as usize);
        });

        let pixels_u8: Vec<u8> = self.pixels.iter().flat_map(|p| p.0).collect();
        Canvas {
            width,
            height,
            pixels: pixels_u8,
        }
    }

    fn push_state(&self, state: &ChromeState) {
        let c = &self.component;
        c.set_shutter(state.shutter.clone().into());
        c.set_iso(state.iso.clone().into());
        c.set_ev(state.ev.clone().into());
        c.set_wb(state.wb.clone().into());
        c.set_link_state(state.link_state.into());
        c.set_is_recording(state.is_recording);
        c.set_rec_elapsed(state.rec_elapsed.clone().into());
        c.set_follow_label(if state.follow_on { "ON" } else { "OFF" }.into());
        c.set_format_label(state.format_label.clone().into());
        c.set_expo_label(state.expo_label.clone().into());
        c.set_battery_text(state.battery_text.clone().into());
        c.set_battery_percent(state.battery_percent);
        c.set_storage_text(state.storage_text.clone().into());
        c.set_zoom_value(state.zoom);
        c.set_zoom_label(state.zoom_label.clone().into());
        c.set_mode_selected(state.mode.min(MODES.len() - 1) as i32);
        c.set_photo_mode(state.photo_mode);
        c.set_controls_enabled(state.controls_enabled);
        let (fx, fy, fw, fh) = state.fit;
        c.set_fit_x(fx as f32);
        c.set_fit_y(fy as f32);
        c.set_fit_width(fw as f32);
        c.set_fit_height(fh as f32);
        c.set_countdown(
            state
                .countdown
                .map(|seconds| seconds.to_string())
                .unwrap_or_default()
                .into(),
        );
        c.set_fps_text(
            if state.fps_shown > 0 {
                format!("{} FPS", state.fps_shown)
            } else {
                String::new()
            }
            .into(),
        );
        c.set_timecode(state.timecode.clone().into());
        c.set_grid_on(state.grid_on);
        c.set_screen(state.screen.index());

        if let Some(library) = &state.library {
            let cells: Vec<MediaCell> = library
                .cells
                .iter()
                .map(|cell| {
                    let thumb = self.thumbs.get(&cell.path).cloned();
                    MediaCell {
                        path: cell.path.clone().into(),
                        title: cell.title.clone().into(),
                        meta: cell.meta.clone().into(),
                        has_thumb: thumb.is_some(),
                        thumb: thumb.unwrap_or_default(),
                        is_video: cell.is_video,
                        starred: cell.starred,
                        cached: cell.cached,
                        selected: cell.selected,
                    }
                })
                .collect();
            c.set_library_cells(ModelRc::new(VecModel::from(cells)));
            c.set_library_tab(library.tab as i32);
            c.set_library_sort(library.sort_label.clone().into());
            c.set_library_status(library.status.clone().into());
            c.set_library_has_selection(library.selection.is_some());
            if let Some(selection) = &library.selection {
                c.set_library_selection(MediaSelection {
                    title: selection.title.clone().into(),
                    meta: selection.meta.clone().into(),
                    is_video: selection.is_video,
                    starred: selection.starred,
                    cached: selection.cached,
                    deletable: selection.deletable,
                    delete_armed: selection.delete_armed,
                    progress: selection.progress.unwrap_or(-1.0),
                    note: selection.note.clone().into(),
                });
            }
        } else if state.screen == Screen::Viewfinder && c.get_library_has_selection() {
            c.set_library_cells(ModelRc::new(VecModel::from(Vec::<MediaCell>::new())));
            c.set_library_has_selection(false);
        }

        if let Some(player) = &state.player {
            c.set_player(SlintPlayerView {
                title: player.title.clone().into(),
                tag: player.tag.clone().into(),
                position_label: player.position_label.clone().into(),
                duration_label: player.duration_label.clone().into(),
                progress: player.progress,
                playing: player.playing,
                assists: player.assists.clone().into(),
                is_photo: player.is_photo,
            });
        }

        let strings = |items: &[String]| -> ModelRc<SharedString> {
            ModelRc::new(VecModel::from(
                items
                    .iter()
                    .map(|item| SharedString::from(item.as_str()))
                    .collect::<Vec<_>>(),
            ))
        };
        match &state.sheet {
            Some(sheet) => {
                c.set_sheet_title(sheet.title.clone().into());
                c.set_sheet_tabs(strings(&sheet.tabs));
                c.set_sheet_tab(sheet.tab as i32);
                let rows: Vec<SheetRow> = sheet
                    .rows
                    .iter()
                    .map(|row| SheetRow {
                        title: row.title.clone().into(),
                        options: strings(&row.options),
                        selected: row.selected.map_or(-1, |i| i as i32),
                        enabled: row.enabled,
                    })
                    .collect();
                c.set_sheet_rows(ModelRc::new(VecModel::from(rows)));
                c.set_sheet_open(true);
            }
            None => {
                if c.get_sheet_open() {
                    c.set_sheet_open(false);
                    c.set_sheet_rows(ModelRc::new(VecModel::from(Vec::<SheetRow>::new())));
                }
            }
        }

        let (msg, failed) = match state.phase {
            Phase::Live => (String::new(), false),
            Phase::Failed(reason) => (reason.to_uppercase(), true),
            other => (other.message().unwrap_or_default(), false),
        };
        c.set_phase_message(msg.into());
        c.set_phase_failed(failed);
    }
}
