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
use slint::{LogicalPosition, PhysicalSize};

use opc_ui::canvas::Canvas;
use opc_ui::hud::Phase;

// Slint's generated component types carry no `Debug`; the workspace lint would flag them.
mod generated {
    #![allow(missing_debug_implementations)]
    slint::include_modules!();
}
use generated::HudOverlay;
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
}

// ── State passed by the shell each frame ─────────────────────────────────────

/// Everything the chrome needs to know for one frame.
#[derive(Debug)]
pub struct ChromeState<'a> {
    pub phase: &'a Phase,
    /// Exposure chips shown in the left column (shutter, ISO, EV, WB).
    pub chip1: String,
    pub chip2: String,
    pub chip3: String,
    pub chip4: String,
    pub link_state: &'a str,
    pub is_recording: bool,
    pub rec_elapsed: String,
    /// Right-column status labels.
    pub battery_text: String,
    pub storage_text: String,
    /// Current zoom (1.0 – 6.0) — drives slider thumb position.
    pub zoom: f32,
    /// Formatted zoom label, e.g. "1.0×".
    pub zoom_label: String,
}

// ── Chrome ───────────────────────────────────────────────────────────────────

pub struct Chrome {
    window: Rc<MinimalSoftwareWindow>,
    component: HudOverlay,
    pixels: Vec<RgbaPixel>,
    size: (u32, u32),
    intents: Rc<RefCell<Vec<ChromeIntent>>>,
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

        Ok(Chrome {
            window,
            component,
            pixels: Vec::new(),
            size: (0, 0),
            intents,
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

    /// Drain all ChromeIntents fired since the last call.
    pub fn drain_intents(&self) -> Vec<ChromeIntent> {
        self.intents.borrow_mut().drain(..).collect()
    }

    /// Whether (x, y) in physical pixels falls inside one of the Slint control
    /// zones.  Used by the shell to decide if a pointer down goes to Slint
    /// rather than starting a tracking-box drag.
    pub fn is_over_control(&self, x: f64, y: f64, _w: u32, h: u32) -> bool {
        let h = h as f64;
        // Bottom bar (80 px): all interactive controls live here
        if y >= h - 80.0 {
            return true;
        }
        // Zoom strip (44 px, starting at y=48)
        if (48.0..=92.0).contains(&y) {
            return true;
        }
        // Left exposure column (60 px wide) — read-only, but don't start tracking boxes there
        if x <= 60.0 {
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
        c.set_chip1(state.chip1.clone().into());
        c.set_chip2(state.chip2.clone().into());
        c.set_chip3(state.chip3.clone().into());
        c.set_chip4(state.chip4.clone().into());
        c.set_link_state(state.link_state.into());
        c.set_is_recording(state.is_recording);
        c.set_rec_elapsed(state.rec_elapsed.clone().into());
        c.set_battery_text(state.battery_text.clone().into());
        c.set_storage_text(state.storage_text.clone().into());
        c.set_zoom_value(state.zoom);
        c.set_zoom_label(state.zoom_label.clone().into());

        let (msg, failed) = match state.phase {
            Phase::Live => (String::new(), false),
            Phase::Failed(reason) => (reason.to_uppercase(), true),
            other => (other.message().unwrap_or_default(), false),
        };
        c.set_phase_message(msg.into());
        c.set_phase_failed(failed);
    }
}
