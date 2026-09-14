//! The desktop shell's camera link.
//!
//! Nothing here decides what goes on the wire. Opcodes, payload bytes, the handshake,
//! and — most importantly — which acknowledgement cursors may move and when all come
//! from `OpenPocketViewCore` through the desktop facade. This crate owns the socket,
//! the clock, and the typing.

mod ble;
mod command;
mod depacketizer;
mod health;
mod packed;
mod pairing;
mod sequence;
mod session;
pub mod softap;
mod status;
mod transport;
mod watchdog;
pub mod wifi;

use std::fmt;

pub use ble::{Advert, BleTransport, Discovered, GattMap, NotificationAssembler};
pub use command::Command;
pub use depacketizer::Depacketizer;
pub use health::FeedHealth;
pub use packed::DumlFrame;
pub use pairing::{PairState, PairStep, Pairing, Reply, PAIR_DEADLINE, STEP_RETRY};
pub use sequence::{Outgoing, Phase, Sequencer, ACK_INTERVAL, HANDSHAKE_DEADLINE, HANDSHAKE_RETRY};
pub use session::{CameraSession, SessionError, SessionEvent};
pub use status::{frame_rate_fps, resolution_name, Status, StatusDecoder};
pub use transport::{
    encode_frame, handshake, is_handshake, pair_approval_ack, pair_set_pin, pair_wake_access_point,
    routing_header, scan_frames, status_string, subscribe, tap_focus, transport_header,
    transport_seq, AckPump, AckWindows, PktType,
};
pub use watchdog::{Recovery, Watchdog};

/// Why the core refused a command or a datagram.
///
/// Not `Eq`: a rejected command can carry a zoom factor or a tracking box.
#[derive(Debug, Clone, PartialEq)]
pub enum CameraError {
    /// The command carried an argument the core does not accept — an ISO index that is
    /// not on the ladder, a colour mode the body does not have.
    Rejected(Command),
    /// A datagram was shorter or stranger than the core could read.
    Malformed,
    /// A string argument contained an interior NUL.
    InvalidText,
    /// The core returned a status this build does not recognise.
    Unknown(i32),
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(command) => write!(f, "the camera core refused {command:?}"),
            Self::Malformed => write!(f, "the camera sent something this build cannot read"),
            Self::InvalidText => write!(f, "text contained an interior NUL"),
            Self::Unknown(code) => write!(f, "unexpected core status {code}"),
        }
    }
}

impl std::error::Error for CameraError {}
