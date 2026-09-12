//! Pointing the camera at something.
//!
//! A drag on screen has to become a box in the camera's own coordinates, and the two are
//! not the same rectangle: the picture is letterboxed inside the window, and the operator
//! may be watching it mirrored. Getting that mapping wrong points the camera somewhere
//! near what they meant, which is worse than not tracking at all.

use opc_camera::Command;

/// Where the picture actually sits inside the window.
///
/// The renderer stretches a 16:9 feed into whatever shape the window is, so a click at
/// the window's centre is the picture's centre, but a click near an edge may be outside
/// the picture entirely.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Fit {
    /// Fits `source` inside `window`, preserving aspect and centring the remainder.
    pub fn letterbox(source: (u32, u32), window: (u32, u32)) -> Self {
        let (source_width, source_height) = (f64::from(source.0), f64::from(source.1));
        let (window_width, window_height) = (f64::from(window.0), f64::from(window.1));
        if source_width <= 0.0
            || source_height <= 0.0
            || window_width <= 0.0
            || window_height <= 0.0
        {
            return Self {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            };
        }
        let scale = (window_width / source_width).min(window_height / source_height);
        let width = source_width * scale;
        let height = source_height * scale;
        Self {
            x: (window_width - width) / 2.0,
            y: (window_height - height) / 2.0,
            width,
            height,
        }
    }

    /// Window coordinates to a fraction of the picture, or `None` outside it.
    pub fn normalise(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if self.width <= 0.0 || self.height <= 0.0 {
            return None;
        }
        let u = (x - self.x) / self.width;
        let v = (y - self.y) / self.height;
        (0.0..=1.0).contains(&u).then_some(())?;
        (0.0..=1.0).contains(&v).then_some(())?;
        Some((u, v))
    }
}

/// A tracking box being dragged out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    origin: (f64, f64),
    current: (f64, f64),
    mirrored: bool,
}

/// Smaller than this and it is a click, not a box.
const MINIMUM_SIDE: f64 = 0.02;

impl Drag {
    /// Starts a drag at a point already normalised to the picture.
    pub fn start(point: (f64, f64), mirrored: bool) -> Self {
        Self {
            origin: point,
            current: point,
            mirrored,
        }
    }

    pub fn extend(&mut self, point: (f64, f64)) {
        self.current = point;
    }

    /// The box as drawn, in picture fractions: `(x, y, width, height)`.
    ///
    /// Dragging up or left is the same box as dragging down or right.
    pub fn rectangle(&self) -> (f64, f64, f64, f64) {
        let x = self.origin.0.min(self.current.0);
        let y = self.origin.1.min(self.current.1);
        let width = (self.current.0 - self.origin.0).abs();
        let height = (self.current.1 - self.origin.1).abs();
        (x, y, width, height)
    }

    /// True when the operator dragged a box rather than clicking.
    pub fn is_box(&self) -> bool {
        let (_, _, width, height) = self.rectangle();
        width >= MINIMUM_SIDE && height >= MINIMUM_SIDE
    }

    /// The command this drag became, or `None` for a click.
    ///
    /// Mirroring is undone here: the operator points at what they see, and the camera is
    /// told where that is on its own sensor.
    pub fn command(&self, id: u16) -> Option<Command> {
        if !self.is_box() {
            return None;
        }
        let (x, y, width, height) = self.rectangle();
        let x = if self.mirrored { 1.0 - x - width } else { x };
        Some(Command::TrackSet {
            id,
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_aspect_fills_the_window() {
        let fit = Fit::letterbox((1280, 720), (640, 360));
        assert_eq!((fit.x, fit.y), (0.0, 0.0));
        assert_eq!((fit.width, fit.height), (640.0, 360.0));
    }

    #[test]
    fn a_tall_window_leaves_bars_above_and_below() {
        let fit = Fit::letterbox((1280, 720), (640, 640));
        assert_eq!(fit.width, 640.0);
        assert_eq!(fit.height, 360.0);
        assert_eq!(fit.y, 140.0);
        assert_eq!(fit.x, 0.0);
    }

    #[test]
    fn a_wide_window_leaves_bars_at_the_sides() {
        let fit = Fit::letterbox((1280, 720), (1600, 720));
        assert_eq!(fit.height, 720.0);
        assert_eq!(fit.x, 160.0);
    }

    #[test]
    fn a_click_in_the_bars_is_not_in_the_picture() {
        let fit = Fit::letterbox((1280, 720), (640, 640));
        assert_eq!(fit.normalise(320.0, 10.0), None, "above the picture");
        assert_eq!(fit.normalise(320.0, 630.0), None, "below it");
        assert!(fit.normalise(320.0, 320.0).is_some(), "the middle is in it");
    }

    #[test]
    fn the_centre_of_the_window_is_the_centre_of_the_picture() {
        let fit = Fit::letterbox((1280, 720), (800, 800));
        let (u, v) = fit.normalise(400.0, 400.0).expect("the centre");
        assert!((u - 0.5).abs() < 1e-9);
        assert!((v - 0.5).abs() < 1e-9);
    }

    #[test]
    fn a_degenerate_window_maps_nothing_rather_than_dividing_by_zero() {
        let fit = Fit::letterbox((1280, 720), (0, 0));
        assert_eq!(fit.normalise(0.0, 0.0), None);
        assert_eq!(Fit::letterbox((0, 0), (640, 360)).width, 0.0);
    }

    #[test]
    fn dragging_backwards_is_the_same_box() {
        let mut forwards = Drag::start((0.2, 0.2), false);
        forwards.extend((0.6, 0.5));
        let mut backwards = Drag::start((0.6, 0.5), false);
        backwards.extend((0.2, 0.2));
        assert_eq!(forwards.rectangle(), backwards.rectangle());
    }

    #[test]
    fn a_click_is_not_a_tracking_box() {
        let drag = Drag::start((0.5, 0.5), false);
        assert!(!drag.is_box());
        assert_eq!(drag.command(1), None);

        let mut tiny = Drag::start((0.5, 0.5), false);
        tiny.extend((0.505, 0.505));
        assert!(!tiny.is_box(), "a shaky click is still a click");
    }

    #[test]
    fn a_box_becomes_the_command_the_camera_expects() {
        let mut drag = Drag::start((0.25, 0.25), false);
        drag.extend((0.75, 0.75));
        match drag.command(7) {
            Some(Command::TrackSet {
                id,
                x,
                y,
                width,
                height,
            }) => {
                assert_eq!(id, 7);
                assert!((x - 0.25).abs() < 1e-6);
                assert!((y - 0.25).abs() < 1e-6);
                assert!((width - 0.5).abs() < 1e-6);
                assert!((height - 0.5).abs() < 1e-6);
            }
            other => panic!("expected a tracking box, got {other:?}"),
        }
    }

    #[test]
    fn a_mirrored_view_points_the_camera_at_what_the_operator_sees() {
        // The operator drags the left third of a mirrored picture. On the sensor that is
        // the right third.
        let mut drag = Drag::start((0.0, 0.4), true);
        drag.extend((0.3, 0.6));
        match drag.command(1) {
            Some(Command::TrackSet { x, width, .. }) => {
                assert!((width - 0.3).abs() < 1e-6);
                assert!((x - 0.7).abs() < 1e-6, "mirrored x should be 1 - x - width");
            }
            other => panic!("expected a tracking box, got {other:?}"),
        }
    }

    #[test]
    fn an_unmirrored_box_is_left_alone() {
        let mut drag = Drag::start((0.0, 0.4), false);
        drag.extend((0.3, 0.6));
        match drag.command(1) {
            Some(Command::TrackSet { x, .. }) => assert!(x.abs() < 1e-6),
            other => panic!("expected a tracking box, got {other:?}"),
        }
    }
}
