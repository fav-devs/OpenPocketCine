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

// Command kinds for `opc_camera_command`, mirroring the header.
pub const OPC_CAM_SESSION_WAKE: i32 = 0;
pub const OPC_CAM_SESSION_KEEPALIVE: i32 = 1;
pub const OPC_CAM_GIMBAL_INIT: i32 = 2;
pub const OPC_CAM_APP_PRESENCE: i32 = 3;
pub const OPC_CAM_LIVE_VIEW_ENABLE: i32 = 4;
pub const OPC_CAM_NANO_LIVE_GATE: i32 = 5;
pub const OPC_CAM_RECORD_START: i32 = 10;
pub const OPC_CAM_RECORD_STOP: i32 = 11;
pub const OPC_CAM_SHOOT_PHOTO: i32 = 12;
pub const OPC_CAM_SET_SHOOTING_MODE: i32 = 13;
pub const OPC_CAM_ZOOM_FACTOR: i32 = 20;
pub const OPC_CAM_ZOOM_LENS: i32 = 21;
pub const OPC_CAM_ZOOM_SLEW: i32 = 22;
pub const OPC_CAM_ZOOM_STOP: i32 = 23;
pub const OPC_CAM_GIMBAL_RECENTER: i32 = 30;
pub const OPC_CAM_GIMBAL_FLIP: i32 = 31;
pub const OPC_CAM_GIMBAL_FOLLOW: i32 = 32;
pub const OPC_CAM_GIMBAL_FPV: i32 = 33;
pub const OPC_CAM_GIMBAL_STICK: i32 = 34;
pub const OPC_CAM_GIMBAL_SPEED: i32 = 35;
pub const OPC_CAM_GIMBAL_TIMED_STOP: i32 = 36;
pub const OPC_CAM_GIMBAL_PARAMS_GET: i32 = 37;
pub const OPC_CAM_GIMBAL_TILT_LOCK: i32 = 38;
pub const OPC_CAM_TRACK_SET: i32 = 40;
pub const OPC_CAM_TRACK_CLEAR: i32 = 41;
pub const OPC_CAM_TRACK_POLL: i32 = 42;
pub const OPC_CAM_FOCUS_TRACK_SET: i32 = 43;
pub const OPC_CAM_FOCUS_TRACK_GET: i32 = 44;
pub const OPC_CAM_SET_ISO_INDEX: i32 = 50;
pub const OPC_CAM_SET_ISO_LIMIT: i32 = 51;
pub const OPC_CAM_SET_SHUTTER: i32 = 52;
pub const OPC_CAM_SET_EV: i32 = 53;
pub const OPC_CAM_SET_WB_AUTO: i32 = 54;
pub const OPC_CAM_SET_WB_CUSTOM: i32 = 55;
pub const OPC_CAM_SET_COLOR_MODE: i32 = 56;
pub const OPC_CAM_SET_FOCUS_MODE: i32 = 57;
pub const OPC_CAM_SET_VIDEO_FORMAT: i32 = 58;
pub const OPC_CAM_SET_FOV: i32 = 59;
pub const OPC_CAM_PARAM_GET: i32 = 60;
pub const OPC_CAM_GET_WIFI_SSID: i32 = 61;
pub const OPC_CAM_GET_WIFI_PASSWORD: i32 = 62;
pub const OPC_CAM_ENTER_PLAYBACK: i32 = 63;
pub const OPC_CAM_EXIT_PLAYBACK: i32 = 64;
pub const OPC_CAM_SET_EXPO_MODE: i32 = 65;
pub const OPC_CAM_SET_AUDIO_CHANNEL: i32 = 66;
pub const OPC_CAM_SET_VOCAL_BOOST: i32 = 67;
pub const OPC_CAM_MEDIA_LIST: i32 = 68;
pub const OPC_CAM_MEDIA_LIST_TRIGGER: i32 = 69;
pub const OPC_CAM_MEDIA_DELETE: i32 = 70;
pub const OPC_CAM_MEDIA_FAVORITE: i32 = 71;
pub const OPC_CAM_PLAYBACK_SPECIAL: i32 = 72;
pub const OPC_CAM_GIMBAL_TIMED_TARGET: i32 = 73;
/// Mimo's tap-to-focus burst, one frame each.
pub const OPC_CAM_TAP_FOCUS_PREPARE: i32 = 74;
pub const OPC_CAM_TAP_FOCUS_POINT: i32 = 75;
pub const OPC_CAM_TAP_FOCUS_HINT: i32 = 76;
pub const OPC_CAM_TAP_FOCUS_COMMIT: i32 = 77;
/// Audio DSP: GET the blob; wind and directional patch `@2` of it and SET it back.
pub const OPC_CAM_AUDIO_DSP_GET: i32 = 78;
pub const OPC_CAM_AUDIO_WIND: i32 = 79;
pub const OPC_CAM_AUDIO_DIRECTIONAL: i32 = 80;
/// A `0x02/0xA5` tracking poll reply, as `opc_tracking_poll` reads it.
pub const OPC_TRACKING_UNKNOWN: i32 = -1;
pub const OPC_TRACKING_IDLE: i32 = 0;
pub const OPC_TRACKING_LOCKED: i32 = 1;
pub const OPC_TRACKING_LOCKED_BOX: i32 = 2;

/// `CameraSetMailbox` decisions, as `opc_mailbox_*` return them.
pub const OPC_MAILBOX_LAUNCH: i32 = 0;
pub const OPC_MAILBOX_COALESCE: i32 = 1;
pub const OPC_MAILBOX_ACK_ACCEPT: i32 = 0;
pub const OPC_MAILBOX_ACK_ACCEPT_LATE: i32 = 1;
pub const OPC_MAILBOX_ACK_DROP_SUPERSEDED: i32 = 2;
pub const OPC_MAILBOX_ACK_DROP_UNKNOWN: i32 = 3;
pub const OPC_MAILBOX_TIMEOUT_SUBSCRIBE_MATCHES: i32 = 0;
pub const OPC_MAILBOX_TIMEOUT_WAIT_LATE: i32 = 1;
pub const OPC_MAILBOX_TIMEOUT_LAUNCH_PENDING: i32 = 2;
pub const OPC_MAILBOX_TIMEOUT_IDLE: i32 = 3;
pub const OPC_MAILBOX_PENDING_IMMEDIATE: i32 = 0;
pub const OPC_MAILBOX_PENDING_AFTER_HOLD: i32 = 1;
pub const OPC_MAILBOX_PENDING_NONE: i32 = 2;
/// What a zoom write needs first, as `opc_zoom_hop` returns it.
pub const OPC_ZOOM_HOP_NONE: i32 = 0;
pub const OPC_ZOOM_HOP_COLOR: i32 = 1;
pub const OPC_ZOOM_HOP_BLOCKED: i32 = 2;

pub const OPC_PKT_HANDSHAKE: u8 = 0x00;
pub const OPC_PKT_TELEMETRY: u8 = 0x01;
pub const OPC_PKT_VIDEO: u8 = 0x02;
pub const OPC_PKT_ACKED_DATA: u8 = 0x03;
pub const OPC_PKT_WINDOW_ACK: u8 = 0x04;
pub const OPC_PKT_COMMAND: u8 = 0x05;

pub const OPC_STATUS_LIST_CAP: usize = 32;

/// What the HUD shows. `-1` means the camera has not said; fields that can legitimately
/// be negative carry a separate `has_` flag.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpcCameraStatus {
    pub battery_percent: i32,
    pub charging: i32,
    pub docked: i32,
    pub is_recording: i32,
    pub in_playback: i32,
    pub record_elapsed_sec: i32,
    pub record_remaining_sec: i32,
    pub shooting_mode: i32,
    pub iso: i32,
    pub iso_index: i32,
    pub iso_limit: i32,
    pub ev_thirds: i32,
    pub has_ev: i32,
    pub shutter_denom: i32,
    pub fps: i32,
    pub video_resolution: i32,
    pub video_frame_rate: i32,
    pub color_mode: i32,
    pub expo_mode: i32,
    pub white_balance_kelvin: i32,
    pub white_balance_tint: i32,
    pub has_white_balance_tint: i32,
    pub focus_mode: i32,
    pub focus_track: i32,
    pub storage_free_mb: i32,
    pub storage_total_mb: i32,
    pub zoom_hundredths: i32,
    pub available_shutter_count: i32,
    pub available_iso_count: i32,
    pub available_format_count: i32,
    pub available_color_count: i32,
    pub reserved: i32,
    pub gimbal_yaw_tenth: i32,
    pub gimbal_pitch_tenth: i32,
    pub gimbal_native_pitch_tenth: i32,
    pub gimbal_attitude_seq: i32,
    pub available_shutter: [i32; OPC_STATUS_LIST_CAP],
    pub available_iso: [i32; OPC_STATUS_LIST_CAP],
    pub available_format_resolution: [i32; OPC_STATUS_LIST_CAP],
    pub available_format_frame_rate: [i32; OPC_STATUS_LIST_CAP],
    pub available_color: [i32; OPC_STATUS_LIST_CAP],
    pub wind_nr: i32,
    pub directional_audio: i32,
    pub audio_dsp_blob_count: i32,
    pub audio_dsp_blob: [i32; OPC_STATUS_LIST_CAP],
}

impl Default for OpcCameraStatus {
    fn default() -> Self {
        // Safety: every field is a plain integer or an array of them.
        unsafe { std::mem::zeroed() }
    }
}

impl std::fmt::Debug for OpcCameraStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcCameraStatus")
            .field("battery_percent", &self.battery_percent)
            .field("is_recording", &self.is_recording)
            .finish_non_exhaustive()
    }
}

pub const OPC_WATCHDOG_NONE: i32 = 0;
pub const OPC_WATCHDOG_RESEND_ENABLE: i32 = 1;
pub const OPC_WATCHDOG_REBUILD_DECODER: i32 = 2;
pub const OPC_WATCHDOG_REOPEN_DATALINK: i32 = 3;
pub const OPC_WATCHDOG_FULL_REJOIN: i32 = 4;

/// What the shell knows about the feed right now.
///
/// Ages are seconds, and a **negative** age means never seen — zero is a real age.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpcWatchdogSnapshot {
    pub now: f64,
    pub last_decoded_frame_age: f64,
    pub last_video_packet_age: f64,
    pub last_access_unit_age: f64,
    pub last_status_age: f64,
    pub last_ble_notify_age: f64,
    pub seconds_since_last_rebuild: f64,
    pub seconds_since_last_enable: f64,
    pub seconds_since_focus_track_set: f64,
    pub seconds_since_zoom_set: f64,
    pub seconds_since_gimbal_throw: f64,
    pub seconds_since_camera_set: f64,
    pub flow_healthy: i32,
    pub path_ready: i32,
    pub has_format: i32,
    pub decoder_failed: i32,
    pub live: i32,
    pub saw_picture: i32,
    pub tcp_poke_ready: i32,
    pub displayed_image_removed: i32,
    pub had_video: i32,
    pub reserved: i32,
}

impl Default for OpcWatchdogSnapshot {
    fn default() -> Self {
        // Nothing seen yet; every age absent.
        Self {
            now: 0.0,
            last_decoded_frame_age: -1.0,
            last_video_packet_age: -1.0,
            last_access_unit_age: -1.0,
            last_status_age: -1.0,
            last_ble_notify_age: -1.0,
            seconds_since_last_rebuild: -1.0,
            seconds_since_last_enable: -1.0,
            seconds_since_focus_track_set: -1.0,
            seconds_since_zoom_set: -1.0,
            seconds_since_gimbal_throw: -1.0,
            seconds_since_camera_set: -1.0,
            flow_healthy: 1,
            path_ready: 1,
            has_format: 0,
            decoder_failed: 0,
            live: 0,
            saw_picture: 0,
            tcp_poke_ready: 0,
            displayed_image_removed: 0,
            had_video: 0,
            reserved: 0,
        }
    }
}

/// The three window cursors a pktType-0x04 acknowledgement carries.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpcAckWindows {
    pub video: u32,
    pub acked_data: u32,
    pub extra: u32,
    pub has_acked_data: i32,
    pub has_extra: i32,
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

    /// A false-colour lattice as a cube handle: `paint` non-zero for the zone colours,
    /// zero for the weight. Null for a scale the core does not know.
    pub fn opc_false_color_cube(scale: i32, color_mode: i32, iso: i32, paint: i32) -> *mut c_void;
    /// Writes four floats: zebra highlight, midtone centre and half-width on the feed's
    /// axis, and the peaking gate scale.
    pub fn opc_assist_scalars(
        color_mode: i32,
        iso: i32,
        highlight_ire: f32,
        midtone_ire: f32,
        out: *mut f32,
    ) -> i32;
    /// The opcode key of the frame `opc_camera_command` would build, or -1.
    pub fn opc_camera_command_key(
        kind: i32,
        ints: *const i32,
        int_count: usize,
        reals: *const f64,
        real_count: usize,
    ) -> i32;
    pub fn opc_duml_opcode_key(set: i32, cmd: i32) -> i32;
    pub fn opc_duml_is_live_control(key: i32) -> i32;

    /// The core's `CameraSetMailbox`, one per datalink.
    pub fn opc_mailbox_new() -> *mut c_void;
    pub fn opc_mailbox_destroy(handle: *mut c_void);
    pub fn opc_mailbox_reset(handle: *mut c_void);
    pub fn opc_mailbox_offer(handle: *mut c_void, key: i32, urgent: i32, now: f64) -> i32;
    pub fn opc_mailbox_begin_launch(handle: *mut c_void, key: i32, now: f64);
    pub fn opc_mailbox_note_transmit(handle: *mut c_void, key: i32, seq: i32);
    pub fn opc_mailbox_decide_ack(handle: *mut c_void, key: i32, seq: i32) -> i32;
    pub fn opc_mailbox_timeout(handle: *mut c_void, key: i32, subscribe_matches: i32) -> i32;
    pub fn opc_mailbox_pending_launch(handle: *mut c_void, key: i32, now: f64) -> i32;
    pub fn opc_mailbox_hold_remaining(handle: *mut c_void, key: i32, now: f64) -> f64;
    pub fn opc_mailbox_pipelines(key: i32) -> i32;

    /// Zoom rules per body: the chip stops, and what a write needs first.
    pub fn opc_zoom_stops(
        model_id: i32,
        resolution: i32,
        shooting_mode: i32,
        out: *mut f64,
        capacity: usize,
    ) -> i32;
    pub fn opc_zoom_hop(factor: f64, color_mode: i32, is_recording: i32, out_mode: *mut i32)
        -> i32;
    pub fn opc_zoom_restore_dlog2(factor: f64) -> i32;

    /// Reads a tracking poll reply; `out_box` gets four floats when a box is carried.
    pub fn opc_tracking_poll(payload: *const u8, count: usize, out_box: *mut f32) -> i32;
    pub fn opc_model_supports_tap_focus(model_id: i32) -> i32;

    /// The scale's legend, one `label<TAB>r<TAB>g<TAB>b` line per zone.
    pub fn opc_false_color_legend(
        scale: i32,
        color_mode: i32,
        iso: i32,
        out: *mut u8,
        capacity: usize,
    ) -> i64;

    pub fn opc_camera_command(
        kind: i32,
        seq: u16,
        ints: *const i32,
        int_count: usize,
        reals: *const f64,
        real_count: usize,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_camera_tap_focus(x: f32, y: f32, seq: u16, out: *mut u8, capacity: usize) -> i64;
    pub fn opc_camera_subscribe(
        key: *const c_char,
        sub_id: u32,
        seq: u16,
        out: *mut u8,
        capacity: usize,
    ) -> i64;

    pub fn opc_duml_transport_header(
        pkt_type: u8,
        payload_length: usize,
        session_id: u16,
        seq: u16,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_duml_routing_header(
        seq: u16,
        command_counter: u8,
        drone: i32,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_duml_handshake(
        session_id: u16,
        seq: u16,
        base_seq: u16,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_duml_is_handshake(datagram: *const u8, count: usize) -> i32;
    pub fn opc_duml_transport_seq(datagram: *const u8, count: usize, out: *mut u16) -> i32;
    pub fn opc_duml_scan_frames(raw: *const u8, count: usize, out: *mut u8, capacity: usize)
        -> i64;
    #[allow(clippy::too_many_arguments)]
    pub fn opc_duml_encode(
        sender: u8,
        receiver: u8,
        seq: u16,
        flags: u8,
        cmd_set: u8,
        cmd_id: u8,
        payload: *const u8,
        payload_count: usize,
        out: *mut u8,
        capacity: usize,
    ) -> i64;

    pub fn opc_ack_create() -> *mut c_void;
    pub fn opc_ack_destroy(handle: *mut c_void);
    pub fn opc_ack_advance(handle: *mut c_void, datagram: *const u8, count: usize) -> i32;
    pub fn opc_ack_saw_video(handle: *mut c_void) -> i32;
    pub fn opc_ack_read(handle: *mut c_void, out: *mut OpcAckWindows) -> i32;
    pub fn opc_ack_payload(
        handle: *mut c_void,
        base_seq: u16,
        out: *mut u8,
        capacity: usize,
    ) -> i64;

    pub fn opc_depacketizer_create() -> *mut c_void;
    pub fn opc_depacketizer_destroy(handle: *mut c_void);
    pub fn opc_depacketizer_feed(
        handle: *mut c_void,
        payload: *const u8,
        count: usize,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_depacketizer_take(handle: *mut c_void, out: *mut u8, capacity: usize) -> i64;
    pub fn opc_depacketizer_reset(handle: *mut c_void);
    pub fn opc_depacketizer_dropped(handle: *mut c_void) -> i32;

    pub fn opc_softap_remote_port() -> u16;
    pub fn opc_softap_host(out: *mut u8, capacity: usize) -> i64;
    pub fn opc_softap_may_bind_local_port(port: u16) -> i32;
    pub fn opc_softap_is_associated(ipv4: *const c_char) -> i32;
    pub fn opc_softap_is_path_ready(addresses: *const c_char) -> i32;
    pub fn opc_softap_is_camera_ssid(ssid: *const c_char) -> i32;

    pub fn opc_ble_gatt_uuids(out: *mut u8, capacity: usize) -> i64;
    pub fn opc_ble_advert_decode(
        payload: *const u8,
        count: usize,
        model_id: *mut i32,
        new_format: *mut i32,
        raw_product_type: *mut i32,
    ) -> i32;
    pub fn opc_ble_assembler_create() -> *mut c_void;
    pub fn opc_ble_assembler_destroy(handle: *mut c_void);
    pub fn opc_ble_assembler_append(
        handle: *mut c_void,
        bytes: *const u8,
        count: usize,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_ble_assembler_take(handle: *mut c_void, out: *mut u8, capacity: usize) -> i64;
    pub fn opc_duml_status_string(
        payload: *const u8,
        count: usize,
        out: *mut u8,
        capacity: usize,
    ) -> i64;

    pub fn opc_pair_set_pin(
        pin: *const c_char,
        identifier: *const c_char,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_pair_approval_ack(seq: u16, out: *mut u8, capacity: usize) -> i64;
    pub fn opc_pair_wake_access_point(out: *mut u8, capacity: usize) -> i64;

    pub fn opc_join_deadline_seconds() -> f64;
    pub fn opc_join_retry_pause_seconds() -> f64;
    pub fn opc_join_should_retry(seconds_left: f64) -> i32;
    pub fn opc_join_is_on_target(current_ssid: *const c_char, target: *const c_char) -> i32;
    pub fn opc_join_ssid_to_kick(
        current_ssid: *const c_char,
        target: *const c_char,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
    pub fn opc_join_frequency_hint(out: *mut u8, capacity: usize) -> i64;

    pub fn opc_watchdog_create() -> *mut c_void;
    pub fn opc_watchdog_destroy(handle: *mut c_void);
    pub fn opc_watchdog_tick(handle: *mut c_void, snapshot: *const OpcWatchdogSnapshot) -> i32;
    pub fn opc_watchdog_stage(handle: *mut c_void, out: *mut u8, capacity: usize) -> i64;
    pub fn opc_watchdog_stall_threshold() -> f64;

    pub fn opc_status_create(model_id: i32) -> *mut c_void;
    pub fn opc_status_destroy(handle: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    pub fn opc_status_apply_frame(
        handle: *mut c_void,
        sender: u8,
        receiver: u8,
        seq: u16,
        flags: u8,
        cmd_set: u8,
        cmd_id: u8,
        payload: *const u8,
        count: usize,
    ) -> i32;
    pub fn opc_status_apply_push(handle: *mut c_void, payload: *const u8, count: usize) -> i32;
    pub fn opc_status_read(handle: *mut c_void, out: *mut OpcCameraStatus) -> i32;
    pub fn opc_status_timecode(handle: *mut c_void, out: *mut u8, capacity: usize) -> i64;
    pub fn opc_status_firmware(handle: *mut c_void, out: *mut u8, capacity: usize) -> i64;
    pub fn opc_status_subscribe_keys(out: *mut u8, capacity: usize) -> i64;

    /// Decodes a catalogue page (counter 1 blob, counter 2 blob, every chunk merged) to a
    /// JSON array of media records. Reports the size needed like every other emitter.
    pub fn opc_media_decode(
        sd: *const u8,
        sd_count: usize,
        internal: *const u8,
        internal_count: usize,
        merged: *const u8,
        merged_count: usize,
        out: *mut u8,
        capacity: usize,
    ) -> i64;
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
    fn the_status_record_matches_the_header_file() {
        assert_eq!(OPC_STATUS_LIST_CAP, 32);
        assert_eq!(size_of::<OpcCameraStatus>(), 924);
        assert_eq!(offset_of!(OpcCameraStatus, wind_nr), 784);
        assert_eq!(offset_of!(OpcCameraStatus, gimbal_yaw_tenth), 128);
        assert_eq!(offset_of!(OpcCameraStatus, available_shutter), 144);
        assert_eq!(
            offset_of!(OpcCameraStatus, available_color),
            144 + 4 * 32 * 4
        );
    }

    #[test]
    fn the_watchdog_snapshot_matches_the_header_file() {
        assert_eq!(size_of::<OpcWatchdogSnapshot>(), 136);
        assert_eq!(align_of::<OpcWatchdogSnapshot>(), 8);
        assert_eq!(offset_of!(OpcWatchdogSnapshot, flow_healthy), 96);
        assert_eq!(offset_of!(OpcWatchdogSnapshot, reserved), 132);
    }

    #[test]
    fn ack_windows_match_the_header_file() {
        assert_eq!(size_of::<OpcAckWindows>(), 20);
        assert_eq!(offset_of!(OpcAckWindows, has_acked_data), 12);
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
