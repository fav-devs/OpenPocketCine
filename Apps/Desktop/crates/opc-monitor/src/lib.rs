//! The desktop viewfinder.
//!
//! Only the part that decides things lives here. The window and the camera link are
//! modules of the binary instead, because both reach the Swift core through
//! `opc-camera`, and a test of the viewfinder's behaviour should not need a toolchain
//! that can build it.

pub mod library;
pub mod luts;
pub mod moves;
pub mod sheets;
pub mod shell;

pub use library::{Library, MediaAction, Player};
pub use luts::{LutChoice, LutMenu};
pub use shell::{GimbalMode, Intent, LutRequest, Shell, Toggles, TouchPhase};
