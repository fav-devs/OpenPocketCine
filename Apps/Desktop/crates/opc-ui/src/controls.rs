//! Keys to camera commands.
//!
//! Kept apart from the window so the map is a table rather than a pile of event
//! handlers, and so it can be checked without opening one. A key that means different
//! things depending on what the camera is doing is a key an operator gets wrong under
//! pressure, so the map is flat: one key, one action, always.

use opc_camera::Command;

/// The keys the viewfinder listens for. The window translates its own events into these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Space,
    Escape,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Char(char),
}

/// What a key press does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Send this to the camera.
    Send(Command),
    /// Start or cancel the record countdown.
    ToggleTimer,
    /// Toggle a piece of chrome.
    ToggleZebra,
    TogglePeaking,
    ToggleGrade,
    ToggleMirror,
    /// Clear whatever the camera is following.
    ClearTracking,
    /// Step the body to its next resolution, or its next frame rate at that resolution.
    /// Which one each becomes depends on what the body says it has, so the shell
    /// resolves it against the camera's own list.
    CycleResolution,
    CycleFrameRate,
    /// Hide the chrome entirely, for a clean look at the shot.
    ToggleChrome,
    /// Open or close the settings panel and the exposure sheet.
    ToggleSettings,
    ToggleExposure,
    /// Grab the current frame.
    Still,
    Quit,
}

/// How far one key press moves a continuous control.
const ZOOM_STEP: f64 = 0.5;
const ZOOM_MIN: f64 = 1.0;
const ZOOM_MAX: f64 = 12.0;
/// Gimbal stick centre and throw, in the camera's own units.
pub const STICK_CENTRE: u16 = 1024;
pub const STICK_THROW: u16 = 400;

/// The viewfinder's control surface.
///
/// Holds only what a key press needs to know — the zoom it last asked for — so the
/// camera stays the source of truth for everything else.
#[derive(Debug, Clone)]
pub struct Controls {
    zoom: f64,
}

impl Default for Controls {
    fn default() -> Self {
        Self { zoom: ZOOM_MIN }
    }
}

impl Controls {
    pub fn new() -> Self {
        Self::default()
    }

    /// The zoom the operator last asked for.
    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    /// Follows the camera, so a zoom changed on the body does not fight the next keypress.
    pub fn set_zoom(&mut self, factor: f64) {
        self.zoom = factor.clamp(ZOOM_MIN, ZOOM_MAX);
    }

    /// What this key does, or `None` when it does nothing.
    pub fn press(&mut self, key: Key) -> Option<Action> {
        Some(match key {
            Key::Space => Action::Send(Command::RecordStart),
            Key::Escape => Action::Quit,
            Key::Tab => Action::ToggleSettings,

            // The arrows are the gimbal stick, and a stick has to see every direction
            // at once — two keys held is a diagonal, not the second key winning. The
            // shell keeps that state in a `Stick` and sends it on a timer.
            Key::Left | Key::Right | Key::Up | Key::Down => return None,

            Key::Char(character) => match character.to_ascii_lowercase() {
                'r' => Action::Send(Command::RecordStop),
                't' => Action::ToggleTimer,
                '=' | '+' => {
                    self.zoom = (self.zoom + ZOOM_STEP).min(ZOOM_MAX);
                    Action::Send(Command::ZoomFactor(self.zoom))
                }
                '-' | '_' => {
                    self.zoom = (self.zoom - ZOOM_STEP).max(ZOOM_MIN);
                    Action::Send(Command::ZoomFactor(self.zoom))
                }
                '0' => {
                    self.zoom = ZOOM_MIN;
                    Action::Send(Command::ZoomFactor(self.zoom))
                }
                'c' => Action::Send(Command::GimbalRecenter),
                'f' => Action::Send(Command::GimbalFlip),
                'x' => Action::ClearTracking,
                '[' => Action::CycleResolution,
                ']' => Action::CycleFrameRate,
                'h' => Action::ToggleChrome,
                'z' => Action::ToggleZebra,
                'p' => Action::TogglePeaking,
                'l' => Action::ToggleGrade,
                'm' => Action::ToggleMirror,
                'e' => Action::ToggleExposure,
                's' => Action::Still,
                _ => return None,
            },
        })
    }
}

/// Which directions are held right now.
///
/// The gimbal takes one stick position, not a stream of key presses, so the shell holds
/// this and sends what it adds up to. Opposite directions cancel, which is what a
/// physical stick does when it is pushed both ways.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stick {
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
}

impl Stick {
    /// Records a direction going down or coming up. Returns true when this changed the
    /// stick, so the shell can send only on a change.
    pub fn set(&mut self, key: Key, held: bool) -> bool {
        let slot = match key {
            Key::Left => &mut self.left,
            Key::Right => &mut self.right,
            Key::Up => &mut self.up,
            Key::Down => &mut self.down,
            _ => return false,
        };
        let changed = *slot != held;
        *slot = held;
        changed
    }

    /// Nothing is held, so the gimbal should be resting.
    pub fn is_resting(&self) -> bool {
        *self == Self::default()
    }

    /// Everything held, added up, as a throw in -1…1 on each axis: right and up
    /// positive.
    pub fn target(&self) -> (f64, f64) {
        let axis =
            |negative: bool, positive: bool| f64::from(i32::from(positive) - i32::from(negative));
        (axis(self.left, self.right), axis(self.down, self.up))
    }

    /// Everything held, added up, as the camera's own stick units.
    pub fn command(&self) -> Command {
        let axis = |negative: bool, positive: bool| {
            let throw = i32::from(positive) - i32::from(negative);
            (i32::from(STICK_CENTRE) + throw * i32::from(STICK_THROW)) as u16
        };
        Command::GimbalStick {
            axis0: axis(self.left, self.right),
            axis1: axis(self.down, self.up),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_rolls_and_r_stops() {
        let mut controls = Controls::new();
        assert_eq!(
            controls.press(Key::Space),
            Some(Action::Send(Command::RecordStart))
        );
        assert_eq!(
            controls.press(Key::Char('r')),
            Some(Action::Send(Command::RecordStop))
        );
    }

    fn thrown(stick: &Stick) -> (u16, u16) {
        match stick.command() {
            Command::GimbalStick { axis0, axis1 } => (axis0, axis1),
            other => panic!("expected a stick throw, got {other:?}"),
        }
    }

    #[test]
    fn the_arrows_throw_the_stick_around_its_centre() {
        for (key, expected) in [
            (Key::Left, (STICK_CENTRE - STICK_THROW, STICK_CENTRE)),
            (Key::Right, (STICK_CENTRE + STICK_THROW, STICK_CENTRE)),
            (Key::Up, (STICK_CENTRE, STICK_CENTRE + STICK_THROW)),
            (Key::Down, (STICK_CENTRE, STICK_CENTRE - STICK_THROW)),
        ] {
            let mut stick = Stick::default();
            assert!(stick.set(key, true));
            assert_eq!(thrown(&stick), expected, "for {key:?}");
        }
    }

    #[test]
    fn two_directions_held_is_a_diagonal_not_the_last_one_pressed() {
        let mut stick = Stick::default();
        stick.set(Key::Right, true);
        stick.set(Key::Up, true);
        assert_eq!(
            thrown(&stick),
            (STICK_CENTRE + STICK_THROW, STICK_CENTRE + STICK_THROW)
        );
    }

    #[test]
    fn opposite_directions_cancel_the_way_a_real_stick_does() {
        let mut stick = Stick::default();
        stick.set(Key::Left, true);
        stick.set(Key::Right, true);
        assert_eq!(thrown(&stick), (STICK_CENTRE, STICK_CENTRE));
        assert!(!stick.is_resting(), "both keys are still held");
    }

    #[test]
    fn letting_go_rests_the_stick() {
        let mut stick = Stick::default();
        stick.set(Key::Left, true);
        assert!(stick.set(Key::Left, false));
        assert!(stick.is_resting());
        assert_eq!(thrown(&stick), (STICK_CENTRE, STICK_CENTRE));
    }

    #[test]
    fn a_key_that_is_already_down_does_not_count_as_a_change() {
        let mut stick = Stick::default();
        assert!(stick.set(Key::Up, true));
        assert!(!stick.set(Key::Up, true), "a repeat is not a new throw");
        assert!(!stick.set(Key::Char('z'), true), "not a direction");
    }

    #[test]
    fn the_arrows_do_not_go_through_the_key_map() {
        let mut controls = Controls::new();
        for key in [Key::Left, Key::Right, Key::Up, Key::Down] {
            assert_eq!(controls.press(key), None, "{key:?} belongs to the stick");
        }
    }

    #[test]
    fn zoom_steps_and_stops_at_the_ends() {
        let mut controls = Controls::new();
        assert_eq!(controls.zoom(), 1.0);
        controls.press(Key::Char('='));
        assert_eq!(controls.zoom(), 1.5);
        // Below one is not a zoom the lens has.
        for _ in 0..10 {
            controls.press(Key::Char('-'));
        }
        assert_eq!(controls.zoom(), 1.0);
        for _ in 0..50 {
            controls.press(Key::Char('='));
        }
        assert_eq!(controls.zoom(), 12.0);
    }

    #[test]
    fn zero_snaps_back_to_wide() {
        let mut controls = Controls::new();
        controls.press(Key::Char('='));
        controls.press(Key::Char('='));
        assert_eq!(
            controls.press(Key::Char('0')),
            Some(Action::Send(Command::ZoomFactor(1.0)))
        );
        assert_eq!(controls.zoom(), 1.0);
    }

    #[test]
    fn the_camera_can_correct_the_zoom_we_think_we_are_at() {
        let mut controls = Controls::new();
        // Somebody turned the ring on the body.
        controls.set_zoom(4.0);
        assert_eq!(
            controls.press(Key::Char('=')),
            Some(Action::Send(Command::ZoomFactor(4.5))),
            "the next step should continue from the camera, not from where we were"
        );
    }

    #[test]
    fn a_zoom_the_lens_does_not_have_is_clamped_not_believed() {
        let mut controls = Controls::new();
        controls.set_zoom(99.0);
        assert_eq!(controls.zoom(), 12.0);
        controls.set_zoom(0.1);
        assert_eq!(controls.zoom(), 1.0);
    }

    #[test]
    fn the_chrome_keys_do_not_reach_the_camera() {
        let mut controls = Controls::new();
        for (key, expected) in [
            ('z', Action::ToggleZebra),
            ('p', Action::TogglePeaking),
            ('l', Action::ToggleGrade),
            ('m', Action::ToggleMirror),
            ('s', Action::Still),
            ('t', Action::ToggleTimer),
            ('x', Action::ClearTracking),
            ('[', Action::CycleResolution),
            (']', Action::CycleFrameRate),
            ('h', Action::ToggleChrome),
        ] {
            assert_eq!(controls.press(Key::Char(key)), Some(expected));
        }
    }

    #[test]
    fn an_unmapped_key_does_nothing_rather_than_something_surprising() {
        let mut controls = Controls::new();
        assert_eq!(controls.press(Key::Char('q')), None);
        assert_eq!(controls.press(Key::Char('9')), None);
    }

    #[test]
    fn case_does_not_change_what_a_key_means() {
        let mut controls = Controls::new();
        assert_eq!(controls.press(Key::Char('Z')), Some(Action::ToggleZebra));
        assert_eq!(controls.press(Key::Char('z')), Some(Action::ToggleZebra));
    }
}

/// A throw in -1…1 on each axis as the camera's own stick units, the same scale the
/// keys use at full deflection.
pub fn stick_command(x: f64, y: f64) -> Command {
    let axis = |v: f64| {
        (f64::from(STICK_CENTRE) + v.clamp(-1.0, 1.0) * f64::from(STICK_THROW)).round() as u16
    };
    Command::GimbalStick {
        axis0: axis(x),
        axis1: axis(y),
    }
}

/// First-order follow on the stick throw: the mobile shells' gimbal ramp. A time
/// constant of zero is a passthrough. Transcribed from `GimbalRampFilter` in the core.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RampFilter {
    pub x: f64,
    pub y: f64,
}

impl RampFilter {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Moves toward the target by one step of `dt` seconds with time constant `tau`.
    pub fn tick(&mut self, target_x: f64, target_y: f64, tau: f64, dt: f64) -> (f64, f64) {
        if tau <= 0.0 || dt <= 0.0 {
            self.x = target_x;
            self.y = target_y;
            return (self.x, self.y);
        }
        let alpha = 1.0 - (-dt / tau).exp();
        self.x += (target_x - self.x) * alpha;
        self.y += (target_y - self.y) * alpha;
        (self.x, self.y)
    }

    /// Close enough to the rest position to call it rested.
    pub fn is_settled_at_rest(&self) -> bool {
        self.x.abs() < 0.02 && self.y.abs() < 0.02
    }
}

#[cfg(test)]
mod ramp_tests {
    use super::*;

    #[test]
    fn off_is_a_passthrough_and_soft_follows_first_order() {
        let mut off = RampFilter::default();
        assert_eq!(off.tick(1.0, -1.0, 0.0, 0.04), (1.0, -1.0));
        let mut soft = RampFilter::default();
        let (x, _) = soft.tick(1.0, 0.0, 0.35, 0.04);
        assert!(
            x > 0.0 && x < 0.2,
            "one step is a fraction of the throw: {x}"
        );
        for _ in 0..100 {
            soft.tick(1.0, 0.0, 0.35, 0.04);
        }
        assert!(soft.x > 0.99);
        for _ in 0..100 {
            soft.tick(0.0, 0.0, 0.35, 0.04);
        }
        assert!(soft.is_settled_at_rest());
    }

    #[test]
    fn a_throw_maps_to_the_same_units_as_the_keys() {
        assert_eq!(
            stick_command(1.0, 1.0),
            Command::GimbalStick {
                axis0: STICK_CENTRE + STICK_THROW,
                axis1: STICK_CENTRE + STICK_THROW
            }
        );
        assert_eq!(
            stick_command(0.0, -0.5),
            Command::GimbalStick {
                axis0: STICK_CENTRE,
                axis1: STICK_CENTRE - STICK_THROW / 2
            }
        );
        let mut stick = Stick::default();
        stick.set(Key::Right, true);
        stick.set(Key::Up, true);
        assert_eq!(stick.target(), (1.0, 1.0));
        assert_eq!(stick_command(1.0, 1.0), stick.command());
    }
}
