//! The desktop watcher's relay client.
//!
//! A watcher never opens BLE and never talks to the camera. The phone hosting the
//! session owns the camera datalink; this crate joins that host's Bonjour service over
//! the shared camera Wi-Fi, receives its re-encoded HEVC, and mirrors its telemetry.
//!
//! Protocol decisions are not made here. Framing limits, kind validation, join rules,
//! the retry ladder, and the delivery-delay guard all come from `OpenPocketViewCore`
//! through [`ffi`], so this shell cannot drift from the iOS watcher.

pub mod buffer;
pub mod discovery;
pub mod ffi;
pub mod session;
pub mod transport;

pub use ffi::{
    Command, ControlToken, FrameMeta, Hello, JoinDenied, ProtocolInfo, RelayError, State,
};
pub use session::{JoinTarget, Status, WatcherObserver, WatcherSession};
