//! A game controller, mapped the way the phones map an extended pad (discussion #159):
//! left stick pans and tilts, the triggers zoom, A records, B recentres, X flips,
//! Y clears tracking, the shoulders step the zoom stops, and the D-pad walks ISO and
//! shutter.

/// The buttons the map knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadButton {
    A,
    B,
    X,
    Y,
    LeftShoulder,
    RightShoulder,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

/// What a button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadAction {
    Record,
    Recenter,
    Flip,
    Track,
    ZoomIn,
    ZoomOut,
    IsoUp,
    IsoDown,
    ShutterOpen,
    ShutterClose,
}

impl PadButton {
    pub fn action(self) -> PadAction {
        match self {
            Self::A => PadAction::Record,
            Self::B => PadAction::Recenter,
            Self::X => PadAction::Flip,
            Self::Y => PadAction::Track,
            Self::LeftShoulder => PadAction::ZoomOut,
            Self::RightShoulder => PadAction::ZoomIn,
            Self::DpadUp => PadAction::IsoUp,
            Self::DpadDown => PadAction::IsoDown,
            Self::DpadLeft => PadAction::ShutterOpen,
            Self::DpadRight => PadAction::ShutterClose,
        }
    }
}

/// The map as a line for the Controls tab.
pub const MAP_HELP: &str =
    "Left stick pans and tilts · triggers zoom · A record · B recentre · X flip · Y clear tracking · LB / RB zoom stop · D-pad ISO and shutter";

/// Neutral inside this on the stick and the triggers.
pub const REST: f64 = 0.08;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_is_the_phones_map() {
        assert_eq!(PadButton::A.action(), PadAction::Record);
        assert_eq!(PadButton::B.action(), PadAction::Recenter);
        assert_eq!(PadButton::X.action(), PadAction::Flip);
        assert_eq!(PadButton::Y.action(), PadAction::Track);
        assert_eq!(PadButton::LeftShoulder.action(), PadAction::ZoomOut);
        assert_eq!(PadButton::RightShoulder.action(), PadAction::ZoomIn);
        assert_eq!(PadButton::DpadUp.action(), PadAction::IsoUp);
        assert_eq!(PadButton::DpadLeft.action(), PadAction::ShutterOpen);
    }
}
