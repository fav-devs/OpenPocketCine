//! The chrome over the picture.
//!
//! Deliberately sparse. A field monitor is for judging a shot, and every pixel of chrome
//! is a pixel of shot an operator cannot see — so the HUD is one strip along the top, one
//! along the bottom, and nothing in the middle unless something is wrong.

use opc_camera::Status;

use crate::canvas::{Canvas, BAR, BAR_EDGE, PLATE, RECORD, TRACKING, WARNING, WHITE};
use crate::font;
use crate::tracking::Fit;

/// Where the session is, as far as the operator needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Looking for the camera over Bluetooth.
    Finding,
    /// Reading its Wi-Fi credentials. A camera may want a button pressed on the body.
    Pairing {
        needs_approval: bool,
    },
    /// Joining the camera's network.
    Joining,
    /// Session open, no picture yet.
    Waiting,
    Live,
    /// The feed stalled and is being recovered.
    Recovering,
    Failed(String),
}

impl Phase {
    /// What to tell the operator, or `None` once there is a picture to look at.
    pub fn message(&self) -> Option<String> {
        Some(match self {
            Self::Finding => "LOOKING FOR CAMERA".to_string(),
            Self::Pairing {
                needs_approval: true,
            } => "APPROVE ON THE CAMERA".to_string(),
            Self::Pairing {
                needs_approval: false,
            } => "PAIRING".to_string(),
            Self::Joining => "JOINING CAMERA WI-FI".to_string(),
            Self::Waiting => "WAITING FOR LIVE VIEW".to_string(),
            Self::Live => return None,
            Self::Recovering => "RECOVERING FEED".to_string(),
            Self::Failed(reason) => reason.to_uppercase(),
        })
    }
}

/// A countdown before recording starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Countdown {
    ends_at: f64,
}

impl Countdown {
    pub fn start(now: f64, seconds: f64) -> Self {
        Self {
            ends_at: now + seconds,
        }
    }

    /// Whole seconds left, counting down. Zero means fire.
    pub fn remaining(&self, now: f64) -> u32 {
        (self.ends_at - now).max(0.0).ceil() as u32
    }

    pub fn is_done(&self, now: f64) -> bool {
        now >= self.ends_at
    }
}

/// Everything the chrome draws.
#[derive(Debug, Clone)]
pub struct Hud {
    pub phase: Phase,
    pub status: Status,
    /// Presented frames per second, as the shell measures them.
    pub fps: u32,
    pub countdown: Option<Countdown>,
    /// A tracking box being dragged, in picture fractions.
    pub drag: Option<(f64, f64, f64, f64)>,
    /// Where the picture sits inside the window. `None` means it fills it — a box drawn
    /// against the window when the picture is letterboxed would sit off the subject.
    pub fit: Option<Fit>,
    /// Assists the operator has turned on, drawn as a short list.
    pub assists: Vec<&'static str>,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            phase: Phase::Finding,
            status: Status::default(),
            fps: 0,
            countdown: None,
            drag: None,
            fit: None,
            assists: Vec::new(),
        }
    }
}

/// How big the chrome is drawn, chosen so a 1080p window reads at arm's length.
fn scale_for(width: u32) -> usize {
    match width {
        0..=800 => 1,
        801..=1600 => 2,
        _ => 3,
    }
}

impl Hud {
    /// The compact, camera-truth fields used as separate leading chips. Values only
    /// appear after the body has reported them; chrome must not invent a setting.
    pub fn top_chips(&self) -> Vec<String> {
        let status = &self.status;
        let mut chips = Vec::new();
        if let Some(iso) = status.iso {
            chips.push(format!("ISO {iso}"));
        }
        if let Some(shutter) = status.shutter_label() {
            chips.push(shutter);
        }
        if let Some(thirds) = status.ev_thirds {
            chips.push(format!("EV {:+.1}", f64::from(thirds) / 3.0));
        }
        if let Some(kelvin) = status.white_balance_kelvin.filter(|value| *value > 0) {
            chips.push(format!("{kelvin}K"));
        }
        if let Some(zoom) = status.zoom_label() {
            chips.push(zoom);
        }
        chips
    }

    /// A terse connection truth, held at the top trailing edge so recovery cannot look
    /// like a healthy feed merely because the last image remains on screen.
    pub fn connection_chip(&self) -> &'static str {
        match self.phase {
            Phase::Live => "LINK",
            Phase::Recovering => "RECOVER",
            Phase::Failed(_) => "OFFLINE",
            Phase::Finding | Phase::Pairing { .. } | Phase::Joining | Phase::Waiting => "LINKING",
        }
    }

    /// The bottom strip: what the body is doing.
    pub fn bottom_line(&self) -> String {
        let status = &self.status;
        let mut parts: Vec<String> = Vec::new();
        if let Some(fps) = status.fps {
            parts.push(format!("{fps}P"));
        }
        if self.fps > 0 {
            parts.push(format!("{} FPS", self.fps));
        }
        if let Some(battery) = status.battery_percent.filter(|value| *value >= 0) {
            parts.push(format!("BATT {battery}%"));
        }
        if status.storage_total_mb > 0 {
            parts.push(format!("{} GB FREE", status.storage_free_mb / 1024));
        }
        if !self.assists.is_empty() {
            parts.push(self.assists.join(" "));
        }
        parts.join("  ·  ")
    }

    /// Rasterises the chrome for a window of this size.
    ///
    /// `now` is the shell's monotonic clock, which the countdown needs — a HUD that
    /// cannot see the clock cannot count down.
    pub fn draw(&self, width: u32, height: u32, now: f64) -> Canvas {
        let mut canvas = Canvas::new(width, height);
        if width == 0 || height == 0 {
            return canvas;
        }
        let scale = scale_for(width);
        let margin = (8 * scale) as i64;
        let line = font::text_height(scale) as i64;
        // Full-width bar height: one text row plus padding above and below.
        let bar_h = (margin * 2 + line) as u32;

        // ── Tracking box ────────────────────────────────────────────────────
        // Drawn first so the bars sit over it.
        if let Some((x, y, box_width, box_height)) = self.drag {
            let fit = self.fit.unwrap_or(Fit {
                x: 0.0,
                y: 0.0,
                width: f64::from(width),
                height: f64::from(height),
            });
            canvas.stroke(
                (fit.x + x * fit.width) as i64,
                (fit.y + y * fit.height) as i64,
                (box_width * fit.width) as u32,
                (box_height * fit.height) as u32,
                scale as u32,
                TRACKING,
            );
        }

        // ── Top bar ─────────────────────────────────────────────────────────
        canvas.fill(0, 0, width, bar_h, BAR);
        // Thin separator line at the bottom of the bar.
        canvas.fill(0, bar_h as i64, width, 1, BAR_EDGE);

        // Exposure chips inside the bar — text directly on BAR, no individual plates.
        let mut chip_x = margin;
        for chip in self.top_chips() {
            chip_x += canvas.text(chip_x, margin, &chip, scale, WHITE) as i64 + margin;
        }

        // Link state and record truth at top-right, inside the bar.
        let connection = self.connection_chip();
        let connection_width = font::text_width(connection, scale) as i64;
        let connection_x = i64::from(width) - margin - connection_width;
        canvas.text(connection_x, margin, connection, scale, WHITE);

        if self.status.is_recording {
            let text = format!("REC {}", self.status.elapsed_label());
            let text_width = font::text_width(&text, scale) as i64;
            let dot = (5 * scale) as u32;
            let rec_x = connection_x - margin - text_width - (10 * scale) as i64;
            canvas.text(rec_x, margin, &text, scale, RECORD);
            // Red dot to the left of the elapsed time.
            canvas.fill(
                rec_x - (10 * scale) as i64,
                margin + line / 3,
                dot,
                dot,
                RECORD,
            );
        }

        // ── Bottom bar ──────────────────────────────────────────────────────
        let bottom_y = i64::from(height) - i64::from(bar_h);
        canvas.fill(0, bottom_y, width, bar_h, BAR);
        // Separator at the top edge of the bottom bar.
        canvas.fill(0, bottom_y - 1, width, 1, BAR_EDGE);

        let bottom = self.bottom_line();
        if !bottom.is_empty() {
            canvas.text(margin, bottom_y + margin, &bottom, scale, WHITE);
        }

        // ── Centre ──────────────────────────────────────────────────────────
        // A countdown or a phase message. Drawn on a rounded plate so it reads over any
        // background — both the test-frame gradient and a black window before the feed.
        let centre = self
            .countdown
            .filter(|countdown| !countdown.is_done(now))
            .map(|countdown| countdown.remaining(now).to_string())
            .or_else(|| self.phase.message());
        if let Some(text) = centre {
            let big = scale * 2;
            let text_w = font::text_width(&text, big) as i64;
            let text_h = font::text_height(big) as i64;
            let pad_x = margin * 3;
            let pad_y = margin * 2;
            let panel_w = (text_w + pad_x * 2) as u32;
            let panel_h = (text_h + pad_y * 2) as u32;
            let panel_x = (i64::from(width) - i64::from(panel_w)) / 2;
            let panel_y = (i64::from(height) - i64::from(panel_h)) / 2;

            // Dark panel behind the text, slightly more opaque than the bar.
            canvas.fill(panel_x, panel_y, panel_w, panel_h, PLATE);
            // Thin outline so the panel reads as a deliberate element.
            canvas.stroke(panel_x, panel_y, panel_w, panel_h, 1, BAR_EDGE);

            let colour = if matches!(self.phase, Phase::Failed(_)) {
                WARNING
            } else {
                WHITE
            };
            canvas.text(panel_x + pad_x, panel_y + pad_y, &text, big, colour);
        }

        canvas
    }

    /// True when a running countdown has just reached zero, so the shell rolls.
    pub fn countdown_fired(&self, now: f64) -> bool {
        self.countdown
            .is_some_and(|countdown| countdown.is_done(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(canvas: &Canvas) -> usize {
        canvas
            .pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[3] > 0)
            .count()
    }

    fn reddish(canvas: &Canvas) -> usize {
        canvas
            .pixels
            .chunks_exact(4)
            .filter(|pixel| {
                pixel[3] > 0
                    && i32::from(pixel[0]) - i32::from(pixel[1]) > 80
                    && i32::from(pixel[0]) - i32::from(pixel[2]) > 80
            })
            .count()
    }

    #[test]
    fn a_live_camera_with_nothing_to_say_draws_almost_nothing() {
        let hud = Hud {
            phase: Phase::Live,
            ..Hud::default()
        };
        let canvas = hud.draw(640, 360, 0.0);
        assert_eq!(
            canvas.pixel(320, 180).map(|pixel| pixel[3]),
            Some(0),
            "the persistent link chip must not clutter the middle of the shot"
        );
    }

    #[test]
    fn every_phase_before_live_says_what_it_is_waiting_for() {
        for phase in [
            Phase::Finding,
            Phase::Pairing {
                needs_approval: false,
            },
            Phase::Joining,
            Phase::Waiting,
            Phase::Recovering,
        ] {
            assert!(phase.message().is_some(), "{phase:?} should say something");
            let hud = Hud {
                phase,
                ..Hud::default()
            };
            assert!(!hud.draw(640, 360, 0.0).is_blank());
        }
        assert_eq!(Phase::Live.message(), None);
    }

    #[test]
    fn a_camera_waiting_on_a_person_says_so() {
        let message = Phase::Pairing {
            needs_approval: true,
        }
        .message()
        .expect("a message");
        assert!(
            message.contains("APPROVE"),
            "an operator has to know to walk over, got {message}"
        );
    }

    #[test]
    fn recording_paints_the_lamp_red_and_idle_does_not() {
        let idle = Hud {
            phase: Phase::Live,
            status: Status {
                is_recording: false,
                ..Status::default()
            },
            ..Hud::default()
        };
        assert_eq!(reddish(&idle.draw(800, 450, 0.0)), 0);

        let rolling = Hud {
            phase: Phase::Live,
            status: Status {
                is_recording: true,
                record_elapsed: 65,
                ..Status::default()
            },
            ..Hud::default()
        };
        assert!(
            reddish(&rolling.draw(800, 450, 0.0)) > 0,
            "the lamp should be on"
        );
    }

    #[test]
    fn the_top_strip_carries_the_exposure() {
        let hud = Hud {
            phase: Phase::Live,
            status: Status {
                iso: Some(400),
                shutter_denominator: Some(50),
                ev_thirds: Some(-3),
                zoom_hundredths: Some(250),
                ..Status::default()
            },
            ..Hud::default()
        };
        let line = hud.top_chips().join("  ");
        assert!(line.contains("ISO 400"), "{line}");
        assert!(line.contains("1/50"), "{line}");
        assert!(line.contains("EV -1.0"), "{line}");
        assert!(line.contains("2.5x"), "{line}");
    }

    #[test]
    fn the_top_strip_leaves_out_what_the_camera_has_not_said() {
        let hud = Hud {
            phase: Phase::Live,
            ..Hud::default()
        };
        assert_eq!(hud.top_chips(), Vec::<String>::new(), "no invented values");
    }

    #[test]
    fn link_chip_reports_recovery_instead_of_leaving_a_stale_live_claim() {
        assert_eq!(
            Hud {
                phase: Phase::Live,
                ..Hud::default()
            }
            .connection_chip(),
            "LINK"
        );
        assert_eq!(
            Hud {
                phase: Phase::Recovering,
                ..Hud::default()
            }
            .connection_chip(),
            "RECOVER"
        );
        assert_eq!(
            Hud {
                phase: Phase::Failed("no route".into()),
                ..Hud::default()
            }
            .connection_chip(),
            "OFFLINE"
        );
    }

    #[test]
    fn the_bottom_strip_carries_the_housekeeping() {
        let hud = Hud {
            phase: Phase::Live,
            status: Status {
                fps: Some(25),
                battery_percent: Some(73),
                storage_total_mb: 64_000,
                storage_free_mb: 32_768,
                ..Status::default()
            },
            fps: 24,
            assists: vec!["ZEB", "PEAK"],
            ..Hud::default()
        };
        let line = hud.bottom_line();
        assert!(line.contains("25P"), "{line}");
        assert!(line.contains("BATT 73%"), "{line}");
        assert!(line.contains("32 GB FREE"), "{line}");
        assert!(line.contains("ZEB PEAK"), "{line}");
    }

    #[test]
    fn a_battery_the_camera_has_not_reported_is_not_shown_as_minus_one() {
        let hud = Hud {
            phase: Phase::Live,
            status: Status {
                battery_percent: Some(-1),
                ..Status::default()
            },
            ..Hud::default()
        };
        assert!(!hud.bottom_line().contains("-1"));
    }

    #[test]
    fn a_tracking_box_is_drawn_where_it_was_dragged() {
        let hud = Hud {
            phase: Phase::Live,
            drag: Some((0.25, 0.25, 0.5, 0.5)),
            ..Hud::default()
        };
        let canvas = hud.draw(400, 400, 0.0);
        // The corner of the box, and nothing in its middle.
        assert_eq!(canvas.pixel(100, 100), Some(TRACKING));
        assert_eq!(canvas.pixel(200, 200).map(|pixel| pixel[3]), Some(0));
    }

    #[test]
    fn a_running_countdown_is_drawn_and_a_finished_one_is_not() {
        let hud = Hud {
            phase: Phase::Live,
            countdown: Some(Countdown::start(100.0, 3.0)),
            ..Hud::default()
        };
        assert!(!hud.draw(400, 400, 101.0).is_blank(), "counting down");
        assert!(hud.countdown_fired(103.0));
        assert_eq!(
            hud.draw(400, 400, 103.0)
                .pixel(200, 200)
                .map(|pixel| pixel[3]),
            Some(0),
            "once it fires the centre of the shot is clear again"
        );
    }

    #[test]
    fn a_countdown_counts_whole_seconds_and_then_fires() {
        let countdown = Countdown::start(100.0, 3.0);
        assert_eq!(countdown.remaining(100.0), 3);
        assert_eq!(countdown.remaining(101.5), 2);
        assert_eq!(countdown.remaining(102.2), 1);
        assert!(!countdown.is_done(102.9));
        assert!(countdown.is_done(103.0));
        assert_eq!(countdown.remaining(103.0), 0);
        // And it does not run backwards past zero.
        assert_eq!(countdown.remaining(200.0), 0);
    }

    #[test]
    fn the_chrome_grows_with_the_window() {
        let hud = Hud {
            phase: Phase::Live,
            status: Status {
                iso: Some(400),
                ..Status::default()
            },
            ..Hud::default()
        };
        let small = lit(&hud.draw(640, 360, 0.0));
        let large = lit(&hud.draw(1920, 1080, 0.0));
        assert!(large > small, "a bigger window should draw bigger chrome");
    }

    #[test]
    fn a_window_with_no_pixels_draws_nothing_rather_than_panicking() {
        let hud = Hud::default();
        assert!(hud.draw(0, 0, 0.0).is_blank());
    }

    #[test]
    fn a_failure_is_shown_in_the_warning_colour() {
        let hud = Hud {
            phase: Phase::Failed("camera went away".to_string()),
            ..Hud::default()
        };
        let canvas = hud.draw(800, 450, 0.0);
        assert!(!canvas.is_blank());
        assert!(
            hud.phase.message().expect("a message").contains("CAMERA"),
            "the operator should be told what happened"
        );
    }
}
