//! Raw bindings to the OpenPocketCine Swift desktop facade.
//!
//! Every record here mirrors `Sources/COpcDesktop/include/opc_desktop_types.h`. The
//! Swift side asserts the same sizes and offsets in `DesktopAbiLayoutTests`, so adding
//! a field without updating both fails on both sides rather than corrupting memory.
//!
//! Nothing in this crate interprets the relay wire format. That is the point: the
//! watcher's framing limits, kind validation, join rules, retry ladder, and JSON shapes
//! all live in `OpenPocketViewCore` and are reached through these entry points.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_void};

pub const OPC_RELAY_TEXT_CAP: usize = 64;
pub const OPC_RELAY_REASON_CAP: usize = 256;
pub const OPC_RELAY_OPTIONS_CAP: usize = 64;

pub const OPC_RELAY_OK: i32 = 1;
pub const OPC_RELAY_NEED_MORE: i32 = 0;
pub const OPC_RELAY_ERR_NULL: i32 = -1;
pub const OPC_RELAY_ERR_MALFORMED: i32 = -2;
pub const OPC_RELAY_ERR_PAYLOAD_TOO_LARGE: i32 = -3;
pub const OPC_RELAY_ERR_UNKNOWN_KIND: i32 = -4;
pub const OPC_RELAY_ERR_OUT_OF_RANGE: i32 = -5;

pub const OPC_RELAY_KIND_HELLO: u8 = 0x01;
pub const OPC_RELAY_KIND_STATE: u8 = 0x02;
pub const OPC_RELAY_KIND_FRAME: u8 = 0x03;
pub const OPC_RELAY_KIND_CONTROL_TOKEN: u8 = 0x04;
pub const OPC_RELAY_KIND_JOIN_DENIED: u8 = 0x05;
pub const OPC_RELAY_KIND_REQUEST_CONTROL: u8 = 0x10;
pub const OPC_RELAY_KIND_RELEASE_CONTROL: u8 = 0x11;
pub const OPC_RELAY_KIND_COMMAND: u8 = 0x12;

pub const OPC_RELAY_ACTION_NONE: i32 = 0;
pub const OPC_RELAY_ACTION_RECONNECT: i32 = 1;
pub const OPC_RELAY_ACTION_EXHAUSTED: i32 = 2;

pub const OPC_RELAY_COMMAND_TOGGLE_RECORDING: i32 = 0;
pub const OPC_RELAY_COMMAND_TAP_FOCUS: i32 = 1;
pub const OPC_RELAY_COMMAND_SET_ISO: i32 = 2;
pub const OPC_RELAY_COMMAND_SET_SHUTTER_DENOM: i32 = 3;
pub const OPC_RELAY_COMMAND_SET_WHITE_BALANCE: i32 = 4;
pub const OPC_RELAY_COMMAND_SET_COLOR: i32 = 5;
pub const OPC_RELAY_COMMAND_SET_ZOOM: i32 = 6;

/// One decoded `[u32be length][u8 kind][payload]` message. Offsets are relative to the
/// buffer handed in, so a payload is read in place.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpcRelayMessageHeader {
    pub kind: u8,
    pub reserved: [u8; 3],
    pub payload_offset: u32,
    pub payload_len: u32,
    pub consumed: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpcRelayBlobSplit {
    pub meta_offset: u32,
    pub meta_len: u32,
    pub hevc_offset: u32,
    pub hevc_len: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OpcRelayFrameMeta {
    pub codec: i32,
    pub is_keyframe: i32,
    pub is_recording: i32,
    pub extra_mirrored: i32,
    pub has_encoded_at: i32,
    pub parameter_set_count: i32,
    pub encoded_at: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpcRelayState {
    pub is_recording: i32,
    pub battery_percent: i32,
    pub allows_control_requests: i32,
    pub is_nano: i32,
    pub has_control_options: i32,
    pub iso_count: i32,
    pub shutter_count: i32,
    pub zoom_count: i32,
    pub iso_indices: [i32; OPC_RELAY_OPTIONS_CAP],
    pub shutter_denominators: [i32; OPC_RELAY_OPTIONS_CAP],
    pub zoom_hundredths: [i32; OPC_RELAY_OPTIONS_CAP],
    pub format: [c_char; OPC_RELAY_TEXT_CAP],
    pub color: [c_char; OPC_RELAY_TEXT_CAP],
    pub zoom: [c_char; OPC_RELAY_TEXT_CAP],
    pub live_fps: [c_char; OPC_RELAY_TEXT_CAP],
    pub camera_name: [c_char; OPC_RELAY_TEXT_CAP],
    pub iso: [c_char; OPC_RELAY_TEXT_CAP],
    pub shutter: [c_char; OPC_RELAY_TEXT_CAP],
    pub camera_model: [c_char; OPC_RELAY_TEXT_CAP],
}

impl Default for OpcRelayState {
    fn default() -> Self {
        // Safety: every field is a plain integer or character array, so an all-zero
        // pattern is a valid instance.
        unsafe { std::mem::zeroed() }
    }
}

impl std::fmt::Debug for OpcRelayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcRelayState")
            .field("is_recording", &self.is_recording)
            .field("battery_percent", &self.battery_percent)
            .finish_non_exhaustive()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpcRelayJoinDenied {
    pub passcode_required: i32,
    pub reason: [c_char; OPC_RELAY_REASON_CAP],
}

impl Default for OpcRelayJoinDenied {
    fn default() -> Self {
        // Safety: plain integer and character storage.
        unsafe { std::mem::zeroed() }
    }
}

impl std::fmt::Debug for OpcRelayJoinDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcRelayJoinDenied")
            .field("passcode_required", &self.passcode_required)
            .finish_non_exhaustive()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OpcRelayControlToken {
    pub holder_is_recipient: i32,
    pub holder_name: [c_char; OPC_RELAY_TEXT_CAP],
}

impl Default for OpcRelayControlToken {
    fn default() -> Self {
        // Safety: plain integer and character storage.
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OpcRelayHello {
    pub version: i32,
    pub host_name: [c_char; OPC_RELAY_TEXT_CAP],
    pub camera_name: [c_char; OPC_RELAY_TEXT_CAP],
}

impl Default for OpcRelayHello {
    fn default() -> Self {
        // Safety: plain integer and character storage.
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpcRelayFocusPoint {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OpcRelayProtocolInfo {
    pub version: i32,
    pub hevc_codec: i32,
    pub max_payload_bytes: i32,
    pub framing_header_bytes: i32,
    pub max_retries: i32,
    pub join_timeout_ms: i32,
    pub silence_timeout_ms: i32,
    pub reserved: i32,
    pub service_type: [c_char; OPC_RELAY_TEXT_CAP],
    pub txt_camera: [c_char; 8],
    pub txt_watchable: [c_char; 8],
}

impl Default for OpcRelayProtocolInfo {
    fn default() -> Self {
        // Safety: plain integer and character storage.
        unsafe { std::mem::zeroed() }
    }
}

/// Opaque per-join policy owning the retry ladder and the delivery-delay guard.
#[repr(C)]
#[derive(Debug)]
pub struct OpcRelayWatcherPolicy {
    _private: [u8; 0],
}

extern "C" {
    pub fn opc_relay_protocol_info(out: *mut OpcRelayProtocolInfo) -> i32;
    pub fn opc_desktop_core_version(out: *mut u8, capacity: usize) -> i64;

    pub fn opc_relay_framing_decode(
        buffer: *const u8,
        count: usize,
        out: *mut OpcRelayMessageHeader,
    ) -> i32;
    pub fn opc_relay_framing_encode(
        kind: u8,
        payload: *const u8,
        payload_count: usize,
        out: *mut u8,
        capacity: usize,
    ) -> i64;

    pub fn opc_relay_frame_blob_decode(
        payload: *const u8,
        count: usize,
        split: *mut OpcRelayBlobSplit,
        meta: *mut OpcRelayFrameMeta,
    ) -> i32;
    pub fn opc_relay_frame_parameter_set(
        meta_json: *const u8,
        count: usize,
        index: i32,
        out: *mut u8,
        capacity: usize,
    ) -> i64;

    pub fn opc_relay_state_decode(json: *const u8, count: usize, out: *mut OpcRelayState) -> i32;
    pub fn opc_relay_join_denied_decode(
        json: *const u8,
        count: usize,
        out: *mut OpcRelayJoinDenied,
    ) -> i32;
    pub fn opc_relay_control_token_decode(
        json: *const u8,
        count: usize,
        out: *mut OpcRelayControlToken,
    ) -> i32;
    pub fn opc_relay_hello_decode(json: *const u8, count: usize, out: *mut OpcRelayHello) -> i32;
    pub fn opc_relay_hello_encode(
        host_name: *const c_char,
        passcode: *const c_char,
        watcher_id: *const c_char,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_relay_command_encode(
        kind: i32,
        a: i32,
        b: i32,
        c: i32,
        d: i32,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_relay_focus_map(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        mirrored: i32,
        out: *mut OpcRelayFocusPoint,
    ) -> i32;

    pub fn opc_relay_policy_create(now: f64) -> *mut c_void;
    pub fn opc_relay_policy_destroy(handle: *mut c_void);
    pub fn opc_relay_policy_connected(handle: *mut c_void, now: f64);
    pub fn opc_relay_policy_received(handle: *mut c_void, now: f64, picture: i32);
    pub fn opc_relay_policy_disconnected(handle: *mut c_void, now: f64) -> i32;
    pub fn opc_relay_policy_tick(handle: *mut c_void, now: f64) -> i32;
    pub fn opc_relay_policy_stop(handle: *mut c_void);
    pub fn opc_relay_policy_retry_count(handle: *mut c_void) -> i32;
    pub fn opc_relay_policy_retry_pending(handle: *mut c_void) -> i32;
    pub fn opc_relay_policy_is_falling_behind(
        handle: *mut c_void,
        has_encoded_at: i32,
        encoded_at: f64,
        received_at: f64,
    ) -> i32;

    pub fn opc_lut_parse(
        text: *const u8,
        count: usize,
        error: *mut u8,
        error_capacity: usize,
    ) -> *mut c_void;
    pub fn opc_lut_builtin(name: *const c_char, size: i32) -> *mut c_void;
    pub fn opc_lut_destroy(handle: *mut c_void);
    pub fn opc_lut_size(handle: *mut c_void) -> i32;
    pub fn opc_lut_rgba(handle: *mut c_void, out: *mut f32, capacity: usize) -> i64;
    pub fn opc_lut_resampled(handle: *mut c_void, target: i32) -> *mut c_void;
    pub fn opc_lut_map(handle: *mut c_void, red: f32, green: f32, blue: f32, out: *mut f32) -> i32;
    pub fn opc_lut_builtin_names(out: *mut u8, capacity: usize) -> i64;
}

/// Reads a NUL-terminated string out of one of the fixed character fields.
pub fn read_text(field: &[c_char]) -> String {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod layout {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    // Paired with `DesktopAbiLayoutTests` on the Swift side. Both must change together.

    #[test]
    fn message_header_matches_the_header_file() {
        assert_eq!(size_of::<OpcRelayMessageHeader>(), 16);
        assert_eq!(align_of::<OpcRelayMessageHeader>(), 4);
        assert_eq!(offset_of!(OpcRelayMessageHeader, kind), 0);
        assert_eq!(offset_of!(OpcRelayMessageHeader, payload_offset), 4);
        assert_eq!(offset_of!(OpcRelayMessageHeader, payload_len), 8);
        assert_eq!(offset_of!(OpcRelayMessageHeader, consumed), 12);
    }

    #[test]
    fn blob_split_and_frame_meta_match_the_header_file() {
        assert_eq!(size_of::<OpcRelayBlobSplit>(), 16);
        assert_eq!(size_of::<OpcRelayFrameMeta>(), 32);
        assert_eq!(align_of::<OpcRelayFrameMeta>(), 8);
        assert_eq!(offset_of!(OpcRelayFrameMeta, encoded_at), 24);
    }

    #[test]
    fn state_matches_the_header_file() {
        assert_eq!(size_of::<OpcRelayState>(), 1312);
        assert_eq!(offset_of!(OpcRelayState, iso_indices), 32);
        assert_eq!(offset_of!(OpcRelayState, format), 800);
    }

    #[test]
    fn small_records_match_the_header_file() {
        assert_eq!(size_of::<OpcRelayJoinDenied>(), 260);
        assert_eq!(size_of::<OpcRelayControlToken>(), 68);
        assert_eq!(size_of::<OpcRelayHello>(), 132);
        assert_eq!(size_of::<OpcRelayFocusPoint>(), 8);
        assert_eq!(size_of::<OpcRelayProtocolInfo>(), 112);
    }

    #[test]
    fn text_fields_stop_at_the_terminator() {
        let mut field = [0 as c_char; OPC_RELAY_TEXT_CAP];
        for (slot, byte) in field.iter_mut().zip(b"Pocket 4 Pro") {
            *slot = *byte as c_char;
        }
        assert_eq!(read_text(&field), "Pocket 4 Pro");
    }
}
