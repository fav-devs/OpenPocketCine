//! Viewfinder chrome.
//!
//! The HUD is rasterised on the CPU into a plain RGBA image that the renderer
//! composites over the picture. That is what makes the chrome testable: every label,
//! lamp and box can be asserted pixel by pixel without a window, a GPU or a camera.
//!
//! Swapping this for a real UI toolkit later means replacing one producer of that
//! image, not unpicking the render pipeline.

pub mod canvas;
pub mod controls;
pub mod font;
pub mod format;
pub mod hud;
pub mod tracking;

pub use canvas::{Canvas, Colour};
pub use controls::{stick_command, Action, Controls, Key, RampFilter, Stick};
pub use format::{next_frame_rate, next_resolution};
pub use hud::{Countdown, Hud, Phase};
pub use tracking::{Drag, Fit};
