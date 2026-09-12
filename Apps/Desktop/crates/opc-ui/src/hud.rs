//! The chrome over the picture.
//!
//! Deliberately sparse. A field monitor is for judging a shot, and every pixel of chrome
//! is a pixel of shot an operator cannot see — so the HUD is one strip along the top, one
//! along the bottom, and nothing in the middle unless something is wrong.

use opc_camera::Status;

use crate::canvas::{Canvas, RECORD, TRACKING, WARNING, WHITE};
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
    /// The top strip: what the camera is set to.
    fn top_line(&self) -> String {
        let status = &self.status;
        let mut parts: Vec<String> = Vec::new();
        if let Some(iso) = status.iso {
            parts.push(format!("ISO {iso}"));
        }
        if let Some(shutter) = status.shutter_label() {
            parts.push(shutter);
        }
        if let Some(thirds) = status.ev_thirds {
            let stops = f64::from(thirds) / 3.0;
            parts.push(format!("EV {stops:+.1}"));
        }
        if let Some(kelvin) = status.white_balance_kelvin.filter(|value| *value > 0) {
            parts.push(format!("{kelvin}K"));
        }
        if let Some(zoom) = status.zoom_label() {
            parts.push(zoom);
        }
        parts.join("  ")
    }

    /// The bottom strip: what the body is doing.
    fn bottom_line(&self) -> String {
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
        parts.join("  ")
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

        // A tracking box first, so the strips sit over it rather than under.
        if let Some((x, y, box_width, box_height)) = self.drag {
            let fit = Fit {
                x: 0.0,
                y: 0.0,
                width: f64::from(width),
                height: f64::from(height),
            };
            canvas.stroke(
                (fit.x + x * fit.width) as i64,
                (fit.y + y * fit.height) as i64,
                (box_width * fit.width) as u32,
                (box_height * fit.height) as u32,
                scale as u32,
                TRACKING,
            );
        }

        let top = self.top_line();
        if !top.is_empty() {
            canvas.label(margin, margin, &top, scale, WHITE);
        }

        // The record lamp sits top-right, where it is visible without reading.
        if self.status.is_recording {
            let text = format!("REC {}", self.status.elapsed_label());
            let text_width = font::text_width(&text, scale) as i64;
            let dot = (5 * scale) as u32;
            let x = i64::from(width) - margin - text_width;
            canvas.label(x, margin, &text, scale, RECORD);
            canvas.fill(x - (10 * scale) as i64, margin + line / 3, dot, dot, RECORD);
        }

        let bottom = self.bottom_line();
        if !bottom.is_empty() {
            canvas.label(
                margin,
                i64::from(height) - margin - line,
                &bottom,
                scale,
                WHITE,
            );
        }

        // The middle stays clear unless there is something to say.
        let centre = self
            .countdown
            .filter(|countdown| !countdown.is_done(now))
            .map(|countdown| countdown.remaining(now).to_string())
            .or_else(|| self.phase.message());
        if let Some(text) = centre {
            let big = scale * 2;
            let text_width = font::text_width(&text, big) as i64;
            canvas.label(
                (i64::from(width) - text_width) / 2,
                (i64::from(height) - font::text_height(big) as i64) / 2,
                &text,
                big,
                if matches!(self.phase, Phase::Failed(_)) {
                    WARNING
                } else {
                    WHITE
                },
            );
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
        assert!(canvas.is_blank(), "the middle of the shot must stay clear");
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
        let line = hud.top_line();
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
        assert_eq!(hud.top_line(), "", "no invented values");
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
        assert!(
            hud.draw(400, 400, 103.0).is_blank(),
            "once it fires the shot is clear again"
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
