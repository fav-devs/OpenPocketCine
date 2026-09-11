//! The UDP datalink's byte math and the window-ACK pump.
//!
//! The pump is the part of this port most likely to go wrong quietly. A camera that is
//! acknowledged badly keeps sending telemetry and keeps answering commands while it
//! stops sending pictures, so the symptom is a live HUD over a black frame rather than
//! an error. Every rule about which cursor may move lives in the core; [`AckPump`] is a
//! handle onto it, not a reimplementation.

use std::ffi::{c_void, CString};

use opc_core_sys as sys;

use crate::packed::{self, DumlFrame};
use crate::CameraError;

/// Transport packet types, as the camera uses them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PktType {
    Handshake,
    /// Unsolicited HUD: battery, gimbal pose, status.
    Telemetry,
    /// The picture.
    Video,
    /// Command replies, including every SET and GET round trip.
    AckedData,
    /// What the pump sends 40 times a second.
    WindowAck,
    Command,
}

impl PktType {
    pub fn raw(self) -> u8 {
        match self {
            Self::Handshake => sys::OPC_PKT_HANDSHAKE,
            Self::Telemetry => sys::OPC_PKT_TELEMETRY,
            Self::Video => sys::OPC_PKT_VIDEO,
            Self::AckedData => sys::OPC_PKT_ACKED_DATA,
            Self::WindowAck => sys::OPC_PKT_WINDOW_ACK,
            Self::Command => sys::OPC_PKT_COMMAND,
        }
    }

    pub fn of(datagram: &[u8]) -> Option<Self> {
        match *datagram.get(6)? {
            sys::OPC_PKT_HANDSHAKE => Some(Self::Handshake),
            sys::OPC_PKT_TELEMETRY => Some(Self::Telemetry),
            sys::OPC_PKT_VIDEO => Some(Self::Video),
            sys::OPC_PKT_ACKED_DATA => Some(Self::AckedData),
            sys::OPC_PKT_WINDOW_ACK => Some(Self::WindowAck),
            sys::OPC_PKT_COMMAND => Some(Self::Command),
            _ => None,
        }
    }
}

/// Runs an emitter twice: once to learn the size, once to fill the buffer.
fn emit<F>(encode: F) -> Result<Vec<u8>, CameraError>
where
    F: Fn(*mut u8, usize) -> i64,
{
    let needed = encode(std::ptr::null_mut(), 0);
    if needed < 0 {
        return Err(CameraError::Unknown(needed as i32));
    }
    let mut out = vec![0u8; needed as usize];
    let written = encode(out.as_mut_ptr(), out.len());
    if written < 0 {
        return Err(CameraError::Unknown(written as i32));
    }
    out.truncate(written as usize);
    Ok(out)
}

/// The 8-byte header every datagram carries.
pub fn transport_header(
    pkt_type: PktType,
    payload_length: usize,
    session_id: u16,
    seq: u16,
) -> Result<Vec<u8>, CameraError> {
    emit(|out, capacity| {
        // Safety: the core only writes into `out`.
        unsafe {
            sys::opc_duml_transport_header(
                pkt_type.raw(),
                payload_length,
                session_id,
                seq,
                out,
                capacity,
            )
        }
    })
}

/// The 12-byte routing header a command datagram needs.
///
/// Both sequence fields are in our own command-sequence space. Getting them wrong drops
/// writes silently while reads keep flowing, which is why this is the core's arithmetic.
pub fn routing_header(seq: u16, command_counter: u8, drone: bool) -> Result<Vec<u8>, CameraError> {
    emit(|out, capacity| {
        // Safety: the core only writes into `out`.
        unsafe {
            sys::opc_duml_routing_header(seq, command_counter, i32::from(drone), out, capacity)
        }
    })
}

/// The 48-byte session open. `base_seq` must be a fresh 8-aligned value per connect.
pub fn handshake(session_id: u16, seq: u16, base_seq: u16) -> Result<Vec<u8>, CameraError> {
    emit(|out, capacity| {
        // Safety: the core only writes into `out`.
        unsafe { sys::opc_duml_handshake(session_id, seq, base_seq, out, capacity) }
    })
}

pub fn is_handshake(datagram: &[u8]) -> bool {
    // Safety: `datagram` outlives the call.
    unsafe { sys::opc_duml_is_handshake(datagram.as_ptr(), datagram.len()) == 1 }
}

pub fn transport_seq(datagram: &[u8]) -> Option<u16> {
    let mut seq = 0u16;
    // Safety: `datagram` outlives the call and `seq` is a live u16.
    let status =
        unsafe { sys::opc_duml_transport_seq(datagram.as_ptr(), datagram.len(), &mut seq) };
    (status == 1).then_some(seq)
}

/// Every CRC-valid DUML frame inside a datagram, wrapper and tunnelling included.
pub fn scan_frames(raw: &[u8]) -> Result<Vec<DumlFrame>, CameraError> {
    let blob = emit(|out, capacity| {
        // Safety: `raw` outlives the call.
        unsafe { sys::opc_duml_scan_frames(raw.as_ptr(), raw.len(), out, capacity) }
    })?;
    Ok(packed::split(&blob)
        .into_iter()
        .filter_map(DumlFrame::parse)
        .collect())
}

/// Encodes one frame, CRC included.
pub fn encode_frame(frame: &DumlFrame) -> Result<Vec<u8>, CameraError> {
    emit(|out, capacity| {
        // Safety: the payload outlives the call.
        unsafe {
            sys::opc_duml_encode(
                frame.sender,
                frame.receiver,
                frame.seq,
                frame.flags,
                frame.cmd_set,
                frame.cmd_id,
                frame.payload.as_ptr(),
                frame.payload.len(),
                out,
                capacity,
            )
        }
    })
}

/// Tap-to-focus is three frames, in order.
pub fn tap_focus(x: f32, y: f32, seq: u16) -> Result<Vec<Vec<u8>>, CameraError> {
    let blob = emit(|out, capacity| {
        // Safety: the core only writes into `out`.
        unsafe { sys::opc_camera_tap_focus(x, y, seq, out, capacity) }
    })?;
    Ok(packed::split(&blob)
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect())
}

/// Subscribes to a camera status stream by its own name.
pub fn subscribe(key: &str, sub_id: u32, seq: u16) -> Result<Vec<u8>, CameraError> {
    let key = CString::new(key).map_err(|_| CameraError::InvalidText)?;
    emit(|out, capacity| {
        // Safety: `key` outlives the call.
        unsafe { sys::opc_camera_subscribe(key.as_ptr(), sub_id, seq, out, capacity) }
    })
}

/// The three cursors, as last reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AckWindows {
    pub video: u16,
    pub acked_data: u16,
    pub extra: u16,
    pub saw_acked_data: bool,
    pub saw_extra: bool,
}

/// Tracks the window cursors for one session.
///
/// Feed it every datagram received; ask it for a payload 40 times a second. It refuses
/// to rewind a cursor once the corresponding stream has been seen, which is the rule
/// that keeps pictures flowing.
#[derive(Debug)]
pub struct AckPump {
    handle: *mut c_void,
    base_seq: u16,
}

impl AckPump {
    pub fn new(base_seq: u16) -> Self {
        // Safety: the core returns a retained handle released in `Drop`.
        Self {
            handle: unsafe { sys::opc_ack_create() },
            base_seq,
        }
    }

    /// The handshake base every unseen cursor falls back to.
    pub fn base_seq(&self) -> u16 {
        self.base_seq
    }

    pub fn set_base_seq(&mut self, base_seq: u16) {
        self.base_seq = base_seq;
    }

    /// Folds one received datagram into the cursors.
    pub fn observe(&mut self, datagram: &[u8]) {
        // Safety: `datagram` outlives the call and the handle is live for `self`.
        unsafe { sys::opc_ack_advance(self.handle, datagram.as_ptr(), datagram.len()) };
    }

    /// True once a real video packet has moved group 0 — the session's first picture.
    pub fn saw_video(&self) -> bool {
        // Safety: the handle is live for the lifetime of `self`.
        unsafe { sys::opc_ack_saw_video(self.handle) == 1 }
    }

    pub fn windows(&self) -> AckWindows {
        let mut raw = sys::OpcAckWindows::default();
        // Safety: `raw` is a live record and the handle is live for `self`.
        unsafe { sys::opc_ack_read(self.handle, &mut raw) };
        AckWindows {
            video: raw.video as u16,
            acked_data: raw.acked_data as u16,
            extra: raw.extra as u16,
            saw_acked_data: raw.has_acked_data != 0,
            saw_extra: raw.has_extra != 0,
        }
    }

    /// The complete pktType-`0x04` datagram, header included, ready to send.
    pub fn datagram(&self, session_id: u16) -> Result<Vec<u8>, CameraError> {
        let payload = emit(|out, capacity| {
            // Safety: the handle is live for the lifetime of `self`.
            unsafe { sys::opc_ack_payload(self.handle, self.base_seq, out, capacity) }
        })?;
        // Window ACKs go out with sequence zero; the cursors are the payload.
        let mut datagram = transport_header(PktType::WindowAck, payload.len(), session_id, 0)?;
        datagram.extend_from_slice(&payload);
        Ok(datagram)
    }
}

impl Drop for AckPump {
    fn drop(&mut self) {
        // Safety: retained by `opc_ack_create` and released exactly once.
        unsafe { sys::opc_ack_destroy(self.handle) }
    }
}

// The handle is plain heap state with no thread affinity, and `&mut self` gates every
// mutating call, so the pump may move between threads but is never shared without a lock.
unsafe impl Send for AckPump {}
