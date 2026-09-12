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

            // The gimbal stick. Held keys repeat, and a release rests it.
            Key::Left => Action::Send(Command::GimbalStick {
                axis0: STICK_CENTRE - STICK_THROW,
                axis1: STICK_CENTRE,
            }),
            Key::Right => Action::Send(Command::GimbalStick {
                axis0: STICK_CENTRE + STICK_THROW,
                axis1: STICK_CENTRE,
            }),
            Key::Up => Action::Send(Command::GimbalStick {
                axis0: STICK_CENTRE,
                axis1: STICK_CENTRE + STICK_THROW,
            }),
            Key::Down => Action::Send(Command::GimbalStick {
                axis0: STICK_CENTRE,
                axis1: STICK_CENTRE - STICK_THROW,
            }),

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
                'z' => Action::ToggleZebra,
                'p' => Action::TogglePeaking,
                'l' => Action::ToggleGrade,
                'm' => Action::ToggleMirror,
                's' => Action::Still,
                _ => return None,
            },
        })
    }

    /// The stick resting at centre, sent when a direction key comes up.
    pub fn release_stick() -> Command {
        Command::GimbalStick {
            axis0: STICK_CENTRE,
            axis1: STICK_CENTRE,
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

    #[test]
    fn the_arrows_throw_the_stick_around_its_centre() {
        let mut controls = Controls::new();
        let mut throw = |key| match controls.press(key) {
            Some(Action::Send(Command::GimbalStick { axis0, axis1 })) => (axis0, axis1),
            other => panic!("expected a stick throw, got {other:?}"),
        };
        assert_eq!(throw(Key::Left).0, STICK_CENTRE - STICK_THROW);
        assert_eq!(throw(Key::Right).0, STICK_CENTRE + STICK_THROW);
        assert_eq!(throw(Key::Up).1, STICK_CENTRE + STICK_THROW);
        assert_eq!(throw(Key::Down).1, STICK_CENTRE - STICK_THROW);
        // Each axis rests while the other is thrown.
        assert_eq!(throw(Key::Left).1, STICK_CENTRE);
    }

    #[test]
    fn letting_go_rests_the_stick() {
        assert_eq!(
            Controls::release_stick(),
            Command::GimbalStick {
                axis0: STICK_CENTRE,
                axis1: STICK_CENTRE
            }
        );
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
