//! The desktop viewfinder.
//!
//! Only the part that decides things lives here. The window and the camera link are
//! modules of the binary instead, because both reach the Swift core through
//! `opc-camera`, and a test of the viewfinder's behaviour should not need a toolchain
//! that can build it.

pub mod assists;
pub mod library;
pub mod luts;
pub mod moves;
pub mod pad;
pub mod prefs;
pub mod scopes;
pub mod sheets;
pub mod shell;
pub mod zoom;

pub use assists::{AssistOptions, AssistTool};
pub use library::{Library, MediaAction, Player};
pub use luts::{LutChoice, LutMenu};
pub use opc_camera::SetOutcome;
pub use pad::{PadAction, PadButton};
pub use sheets::{Part, Prefs, SetupInfo};
pub use shell::{FalseColorKey, GimbalMode, Intent, LutRequest, Shell, Toggles, TouchPhase};
