//! Safe wrappers over the Swift core's C ABI.
//!
//! Each call hands the core a borrowed slice and reads back either offsets into that
//! same slice or a fixed record, so no relay decision is duplicated in Rust.

use std::ffi::CString;
use std::fmt;
use std::ops::Range;
use std::os::raw::c_void;
use std::time::Duration;

use opc_core_sys as sys;

/// Why a core call refused the bytes it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayError {
    /// A pointer the core requires was null, or a length was negative.
    Null,
    /// The payload did not match the shape the core expects.
    Malformed,
    /// A declared length exceeded the relay's payload ceiling.
    PayloadTooLarge,
    /// The host sent a message kind this build does not know.
    UnknownKind,
    /// An index or coordinate fell outside the accepted range.
    OutOfRange,
    /// A string handed to the core contained an interior NUL.
    InvalidText,
    /// The core returned a status this build does not recognise.
    Unknown(i32),
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "the core rejected a null argument"),
            Self::Malformed => write!(f, "the host sent a message this build cannot read"),
            Self::PayloadTooLarge => write!(f, "the host sent an oversized payload"),
            Self::UnknownKind => write!(f, "the host sent an unknown message kind"),
            Self::OutOfRange => write!(f, "value out of range"),
            Self::InvalidText => write!(f, "text contained an interior NUL"),
            Self::Unknown(code) => write!(f, "unexpected core status {code}"),
        }
    }
}

impl std::error::Error for RelayError {}

fn check(code: i32) -> Result<(), RelayError> {
    match code {
        sys::OPC_RELAY_OK => Ok(()),
        sys::OPC_RELAY_ERR_NULL => Err(RelayError::Null),
        sys::OPC_RELAY_ERR_MALFORMED => Err(RelayError::Malformed),
        sys::OPC_RELAY_ERR_PAYLOAD_TOO_LARGE => Err(RelayError::PayloadTooLarge),
        sys::OPC_RELAY_ERR_UNKNOWN_KIND => Err(RelayError::UnknownKind),
        sys::OPC_RELAY_ERR_OUT_OF_RANGE => Err(RelayError::OutOfRange),
        other => Err(RelayError::Unknown(other)),
    }
}

/// Runs an encoder twice: once to learn the size, once to fill the buffer.
fn emit<F>(encode: F) -> Result<Vec<u8>, RelayError>
where
    F: Fn(*mut u8, usize) -> i64,
{
    let needed = encode(std::ptr::null_mut(), 0);
    if needed < 0 {
        return Err(check(needed as i32).unwrap_err());
    }
    let mut out = vec![0u8; needed as usize];
    let written = encode(out.as_mut_ptr(), out.len());
    if written < 0 {
        return Err(check(written as i32).unwrap_err());
    }
    out.truncate(written as usize);
    Ok(out)
}

/// Relay constants owned by the core. The shell must not hardcode these.
#[derive(Debug, Clone)]
pub struct ProtocolInfo {
    pub version: i32,
    pub hevc_codec: i32,
    pub max_payload_bytes: usize,
    pub framing_header_bytes: usize,
    pub max_retries: i32,
    pub join_timeout: Duration,
    pub silence_timeout: Duration,
    pub service_type: String,
    pub txt_camera: String,
    pub txt_watchable: String,
}

impl ProtocolInfo {
    pub fn load() -> Result<Self, RelayError> {
        let mut raw = sys::OpcRelayProtocolInfo::default();
        // Safety: `raw` is a live, correctly sized record.
        check(unsafe { sys::opc_relay_protocol_info(&mut raw) })?;
        Ok(Self {
            version: raw.version,
            hevc_codec: raw.hevc_codec,
            max_payload_bytes: raw.max_payload_bytes.max(0) as usize,
            framing_header_bytes: raw.framing_header_bytes.max(0) as usize,
            max_retries: raw.max_retries,
            join_timeout: Duration::from_millis(raw.join_timeout_ms.max(0) as u64),
            silence_timeout: Duration::from_millis(raw.silence_timeout_ms.max(0) as u64),
            service_type: sys::read_text(&raw.service_type),
            txt_camera: sys::read_text(&raw.txt_camera),
            txt_watchable: sys::read_text(&raw.txt_watchable),
        })
    }

    /// The Bonjour type in the form `mdns-sd` expects.
    pub fn mdns_service_type(&self) -> String {
        format!("{}.local.", self.service_type)
    }
}

/// Build identity of the linked Swift core, for the diagnostics line.
pub fn core_version() -> Result<String, RelayError> {
    // Safety: the core writes at most `capacity` bytes and reports the size it needs.
    let bytes = emit(|out, capacity| unsafe { sys::opc_desktop_core_version(out, capacity) })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// One framed message located inside the caller's buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: u8,
    pub payload: Range<usize>,
    pub consumed: usize,
}

/// Reads the next complete message, or `None` when more bytes are needed.
pub fn decode_message(buffer: &[u8]) -> Result<Option<Message>, RelayError> {
    let mut header = sys::OpcRelayMessageHeader::default();
    // Safety: the pointer and length describe `buffer`, which outlives the call.
    let status =
        unsafe { sys::opc_relay_framing_decode(buffer.as_ptr(), buffer.len(), &mut header) };
    if status == sys::OPC_RELAY_NEED_MORE {
        return Ok(None);
    }
    check(status)?;
    let start = header.payload_offset as usize;
    let end = start + header.payload_len as usize;
    if end > buffer.len() || header.consumed as usize > buffer.len() {
        return Err(RelayError::Malformed);
    }
    Ok(Some(Message {
        kind: header.kind,
        payload: start..end,
        consumed: header.consumed as usize,
    }))
}

pub fn encode_message(kind: u8, payload: &[u8]) -> Result<Vec<u8>, RelayError> {
    emit(|out, capacity| {
        // Safety: `payload` outlives the call; the core writes at most `capacity`.
        unsafe {
            sys::opc_relay_framing_encode(kind, payload.as_ptr(), payload.len(), out, capacity)
        }
    })
}

/// `WatcherRelayFrameMetadata`, flattened.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameMeta {
    pub codec: i32,
    pub is_keyframe: bool,
    pub is_recording: bool,
    pub extra_mirrored: bool,
    pub encoded_at: Option<f64>,
    pub parameter_set_count: i32,
}

/// A frame payload split into its metadata and its access unit.
#[derive(Debug, Clone)]
pub struct FrameBlob {
    pub meta: FrameMeta,
    pub meta_json: Range<usize>,
    pub access_unit: Range<usize>,
}

pub fn decode_frame_blob(payload: &[u8]) -> Result<FrameBlob, RelayError> {
    let mut split = sys::OpcRelayBlobSplit::default();
    let mut meta = sys::OpcRelayFrameMeta::default();
    // Safety: `payload` outlives the call and both out-records are live.
    check(unsafe {
        sys::opc_relay_frame_blob_decode(payload.as_ptr(), payload.len(), &mut split, &mut meta)
    })?;
    let meta_start = split.meta_offset as usize;
    let meta_end = meta_start + split.meta_len as usize;
    let unit_start = split.hevc_offset as usize;
    let unit_end = unit_start + split.hevc_len as usize;
    if meta_end > payload.len() || unit_end > payload.len() {
        return Err(RelayError::Malformed);
    }
    Ok(FrameBlob {
        meta: FrameMeta {
            codec: meta.codec,
            is_keyframe: meta.is_keyframe != 0,
            is_recording: meta.is_recording != 0,
            extra_mirrored: meta.extra_mirrored != 0,
            encoded_at: (meta.has_encoded_at != 0).then_some(meta.encoded_at),
            parameter_set_count: meta.parameter_set_count,
        },
        meta_json: meta_start..meta_end,
        access_unit: unit_start..unit_end,
    })
}

/// Copies the VPS/SPS/PPS blobs a keyframe carries, in order.
pub fn parameter_sets(meta_json: &[u8], count: i32) -> Result<Vec<Vec<u8>>, RelayError> {
    (0..count.max(0))
        .map(|index| {
            emit(|out, capacity| {
                // Safety: `meta_json` outlives the call.
                unsafe {
                    sys::opc_relay_frame_parameter_set(
                        meta_json.as_ptr(),
                        meta_json.len(),
                        index,
                        out,
                        capacity,
                    )
                }
            })
        })
        .collect()
}

/// Host telemetry mirrored onto the watcher's HUD.
#[derive(Debug, Clone, Default)]
pub struct State {
    pub is_recording: bool,
    pub battery_percent: i32,
    pub allows_control_requests: bool,
    pub is_nano: Option<bool>,
    pub format: String,
    pub color: String,
    pub zoom: String,
    pub live_fps: String,
    pub camera_name: String,
    pub iso: String,
    pub shutter: String,
    pub camera_model: String,
    pub iso_indices: Vec<i32>,
    pub shutter_denominators: Vec<i32>,
    pub zoom_hundredths: Vec<i32>,
}

pub fn decode_state(json: &[u8]) -> Result<State, RelayError> {
    let mut raw = sys::OpcRelayState::default();
    // Safety: `json` outlives the call and `raw` is a live record.
    check(unsafe { sys::opc_relay_state_decode(json.as_ptr(), json.len(), &mut raw) })?;
    let take = |values: &[i32], count: i32| {
        values[..count.clamp(0, values.len() as i32) as usize].to_vec()
    };
    Ok(State {
        is_recording: raw.is_recording != 0,
        battery_percent: raw.battery_percent,
        allows_control_requests: raw.allows_control_requests != 0,
        is_nano: match raw.is_nano {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        format: sys::read_text(&raw.format),
        color: sys::read_text(&raw.color),
        zoom: sys::read_text(&raw.zoom),
        live_fps: sys::read_text(&raw.live_fps),
        camera_name: sys::read_text(&raw.camera_name),
        iso: sys::read_text(&raw.iso),
        shutter: sys::read_text(&raw.shutter),
        camera_model: sys::read_text(&raw.camera_model),
        iso_indices: take(&raw.iso_indices, raw.iso_count),
        shutter_denominators: take(&raw.shutter_denominators, raw.shutter_count),
        zoom_hundredths: take(&raw.zoom_hundredths, raw.zoom_count),
    })
}

/// Why a host turned a join away. Only a passcode prompt is recoverable.
#[derive(Debug, Clone)]
pub struct JoinDenied {
    pub reason: String,
    pub passcode_required: bool,
}

pub fn decode_join_denied(json: &[u8]) -> Result<JoinDenied, RelayError> {
    let mut raw = sys::OpcRelayJoinDenied::default();
    // Safety: `json` outlives the call and `raw` is a live record.
    check(unsafe { sys::opc_relay_join_denied_decode(json.as_ptr(), json.len(), &mut raw) })?;
    Ok(JoinDenied {
        reason: sys::read_text(&raw.reason),
        passcode_required: raw.passcode_required != 0,
    })
}

/// Who currently holds camera control.
#[derive(Debug, Clone)]
pub struct ControlToken {
    pub holder_name: String,
    pub holder_is_recipient: bool,
}

impl Default for ControlToken {
    fn default() -> Self {
        Self {
            holder_name: "Host".to_string(),
            holder_is_recipient: false,
        }
    }
}

pub fn decode_control_token(json: &[u8]) -> Result<ControlToken, RelayError> {
    let mut raw = sys::OpcRelayControlToken::default();
    // Safety: `json` outlives the call and `raw` is a live record.
    check(unsafe { sys::opc_relay_control_token_decode(json.as_ptr(), json.len(), &mut raw) })?;
    Ok(ControlToken {
        holder_name: sys::read_text(&raw.holder_name),
        holder_is_recipient: raw.holder_is_recipient != 0,
    })
}

/// The host's acceptance reply.
#[derive(Debug, Clone)]
pub struct Hello {
    pub version: i32,
    pub host_name: String,
    pub camera_name: String,
}

impl Hello {
    /// The title the watcher shows for this feed.
    pub fn title(&self) -> String {
        if self.camera_name.is_empty() {
            self.host_name.clone()
        } else {
            format!("{} · {}", self.host_name, self.camera_name)
        }
    }
}

pub fn decode_hello(json: &[u8]) -> Result<Hello, RelayError> {
    let mut raw = sys::OpcRelayHello::default();
    // Safety: `json` outlives the call and `raw` is a live record.
    check(unsafe { sys::opc_relay_hello_decode(json.as_ptr(), json.len(), &mut raw) })?;
    Ok(Hello {
        version: raw.version,
        host_name: sys::read_text(&raw.host_name),
        camera_name: sys::read_text(&raw.camera_name),
    })
}

pub fn encode_hello(
    host_name: &str,
    passcode: &str,
    watcher_id: &str,
) -> Result<Vec<u8>, RelayError> {
    let host = CString::new(host_name).map_err(|_| RelayError::InvalidText)?;
    let code = CString::new(passcode).map_err(|_| RelayError::InvalidText)?;
    let identifier = CString::new(watcher_id).map_err(|_| RelayError::InvalidText)?;
    emit(|out, capacity| {
        // Safety: all three strings outlive the call.
        unsafe {
            sys::opc_relay_hello_encode(
                host.as_ptr(),
                code.as_ptr(),
                identifier.as_ptr(),
                out,
                capacity,
            )
        }
    })
}

/// Camera writes a watcher may send once the host grants control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    ToggleRecording,
    TapFocus {
        camera_x: i32,
        camera_y: i32,
        coordinate_width: i32,
        coordinate_height: i32,
    },
    SetIso(i32),
    SetShutterDenominator(i32),
    SetWhiteBalance {
        mode: i32,
        kelvin: i32,
        tint: i32,
    },
    SetColor(i32),
    SetZoom(i32),
}

impl Command {
    fn arguments(self) -> (i32, [i32; 4]) {
        match self {
            Self::ToggleRecording => (sys::OPC_RELAY_COMMAND_TOGGLE_RECORDING, [0, 0, 0, 0]),
            Self::TapFocus {
                camera_x,
                camera_y,
                coordinate_width,
                coordinate_height,
            } => (
                sys::OPC_RELAY_COMMAND_TAP_FOCUS,
                [camera_x, camera_y, coordinate_width, coordinate_height],
            ),
            Self::SetIso(value) => (sys::OPC_RELAY_COMMAND_SET_ISO, [value, 0, 0, 0]),
            Self::SetShutterDenominator(value) => {
                (sys::OPC_RELAY_COMMAND_SET_SHUTTER_DENOM, [value, 0, 0, 0])
            }
            Self::SetWhiteBalance { mode, kelvin, tint } => (
                sys::OPC_RELAY_COMMAND_SET_WHITE_BALANCE,
                [mode, kelvin, tint, 0],
            ),
            Self::SetColor(value) => (sys::OPC_RELAY_COMMAND_SET_COLOR, [value, 0, 0, 0]),
            Self::SetZoom(value) => (sys::OPC_RELAY_COMMAND_SET_ZOOM, [value, 0, 0, 0]),
        }
    }
}

pub fn encode_command(command: Command) -> Result<Vec<u8>, RelayError> {
    let (kind, args) = command.arguments();
    emit(|out, capacity| {
        // Safety: the core only writes into `out`.
        unsafe {
            sys::opc_relay_command_encode(kind, args[0], args[1], args[2], args[3], out, capacity)
        }
    })
}

/// Maps a click on the fitted picture to camera focus coordinates.
pub fn map_focus(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    mirrored: bool,
) -> Result<(i32, i32), RelayError> {
    let mut point = sys::OpcRelayFocusPoint::default();
    // Safety: `point` is a live record.
    check(unsafe {
        sys::opc_relay_focus_map(x, y, width, height, i32::from(mirrored), &mut point)
    })?;
    Ok((point.x, point.y))
}

/// What the core says to do after a deadline passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    None,
    Reconnect,
    Exhausted,
}

fn action(code: i32) -> RecoveryAction {
    match code {
        sys::OPC_RELAY_ACTION_RECONNECT => RecoveryAction::Reconnect,
        sys::OPC_RELAY_ACTION_EXHAUSTED => RecoveryAction::Exhausted,
        _ => RecoveryAction::None,
    }
}

/// Owns the core's per-join retry ladder and delivery-delay guard.
#[derive(Debug)]
pub struct WatcherPolicy {
    handle: *mut c_void,
}

impl WatcherPolicy {
    pub fn new(now: f64) -> Self {
        // Safety: the core returns a retained handle released in `Drop`.
        Self {
            handle: unsafe { sys::opc_relay_policy_create(now) },
        }
    }

    pub fn connected(&mut self, now: f64) {
        // Safety: `handle` is live for the lifetime of `self`.
        unsafe { sys::opc_relay_policy_connected(self.handle, now) }
    }

    pub fn received(&mut self, now: f64, picture: bool) {
        // Safety: `handle` is live for the lifetime of `self`.
        unsafe { sys::opc_relay_policy_received(self.handle, now, i32::from(picture)) }
    }

    pub fn disconnected(&mut self, now: f64) -> RecoveryAction {
        // Safety: `handle` is live for the lifetime of `self`.
        action(unsafe { sys::opc_relay_policy_disconnected(self.handle, now) })
    }

    pub fn tick(&mut self, now: f64) -> RecoveryAction {
        // Safety: `handle` is live for the lifetime of `self`.
        action(unsafe { sys::opc_relay_policy_tick(self.handle, now) })
    }

    pub fn stop(&mut self) {
        // Safety: `handle` is live for the lifetime of `self`.
        unsafe { sys::opc_relay_policy_stop(self.handle) }
    }

    pub fn retry_count(&self) -> i32 {
        // Safety: `handle` is live for the lifetime of `self`.
        unsafe { sys::opc_relay_policy_retry_count(self.handle) }
    }

    pub fn retry_pending(&self) -> bool {
        // Safety: `handle` is live for the lifetime of `self`.
        unsafe { sys::opc_relay_policy_retry_pending(self.handle) != 0 }
    }

    /// True once delivery delay has grown past the relay's tolerance.
    pub fn is_falling_behind(&mut self, encoded_at: Option<f64>, received_at: f64) -> bool {
        // Safety: `handle` is live for the lifetime of `self`.
        unsafe {
            sys::opc_relay_policy_is_falling_behind(
                self.handle,
                i32::from(encoded_at.is_some()),
                encoded_at.unwrap_or(0.0),
                received_at,
            ) != 0
        }
    }
}

impl Drop for WatcherPolicy {
    fn drop(&mut self) {
        // Safety: the handle was retained by `opc_relay_policy_create` and is released once.
        unsafe { sys::opc_relay_policy_destroy(self.handle) }
    }
}

// The handle is plain heap state with no thread affinity, and `&mut self` gates every
// mutating call, so it may move between threads but is never shared without a lock.
unsafe impl Send for WatcherPolicy {}
