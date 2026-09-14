// Fixed-layout C types shared across the desktop ABI boundary.
//
// The Swift facade (`Sources/OpenPocketCineDesktopFacade/`) imports this module and
// fills these records; the Rust host (`Apps/Desktop/`) declares matching `#[repr(C)]`
// structs. Neither side parses the relay wire format on its own — every size limit,
// kind check, deadline, and JSON shape stays in `OpenPocketViewCore`.
//
// Layout is asserted from both sides (`DesktopFacadeLayoutTests` in Swift,
// `layout` tests in `opc-core-sys`). Changing a field here means changing both.

#ifndef OPC_DESKTOP_TYPES_H
#define OPC_DESKTOP_TYPES_H

#include <stdint.h>

#define OPC_RELAY_TEXT_CAP 64
#define OPC_RELAY_REASON_CAP 256
#define OPC_RELAY_OPTIONS_CAP 64

// Status codes. Positive is success, zero means the caller must read more bytes.
#define OPC_RELAY_OK 1
#define OPC_RELAY_NEED_MORE 0
#define OPC_RELAY_ERR_NULL (-1)
#define OPC_RELAY_ERR_MALFORMED (-2)
#define OPC_RELAY_ERR_PAYLOAD_TOO_LARGE (-3)
#define OPC_RELAY_ERR_UNKNOWN_KIND (-4)
#define OPC_RELAY_ERR_OUT_OF_RANGE (-5)

// Message kinds, mirroring `WatcherRelayProtocol.Kind`.
#define OPC_RELAY_KIND_HELLO 0x01
#define OPC_RELAY_KIND_STATE 0x02
#define OPC_RELAY_KIND_FRAME 0x03
#define OPC_RELAY_KIND_CONTROL_TOKEN 0x04
#define OPC_RELAY_KIND_JOIN_DENIED 0x05
#define OPC_RELAY_KIND_REQUEST_CONTROL 0x10
#define OPC_RELAY_KIND_RELEASE_CONTROL 0x11
#define OPC_RELAY_KIND_COMMAND 0x12

// `WatcherRelayRecovery.Action`.
#define OPC_RELAY_ACTION_NONE 0
#define OPC_RELAY_ACTION_RECONNECT 1
#define OPC_RELAY_ACTION_EXHAUSTED 2

// `WatcherRelayCommand` cases.
#define OPC_RELAY_COMMAND_TOGGLE_RECORDING 0
#define OPC_RELAY_COMMAND_TAP_FOCUS 1
#define OPC_RELAY_COMMAND_SET_ISO 2
#define OPC_RELAY_COMMAND_SET_SHUTTER_DENOM 3
#define OPC_RELAY_COMMAND_SET_WHITE_BALANCE 4
#define OPC_RELAY_COMMAND_SET_COLOR 5
#define OPC_RELAY_COMMAND_SET_ZOOM 6

/// One decoded `[u32be length][u8 kind][payload]` message. Offsets are relative to the
/// start of the buffer handed in, so the host never copies to read a payload.
typedef struct {
    uint8_t kind;
    uint8_t reserved[3];
    uint32_t payload_offset;
    uint32_t payload_len;
    uint32_t consumed;
} OpcRelayMessageHeader;

/// Where the metadata JSON and the HEVC access unit sit inside a frame payload.
typedef struct {
    uint32_t meta_offset;
    uint32_t meta_len;
    uint32_t hevc_offset;
    uint32_t hevc_len;
} OpcRelayBlobSplit;

/// `WatcherRelayFrameMetadata`. `has_encoded_at` is 0 for senders that omit the clock.
typedef struct {
    int32_t codec;
    int32_t is_keyframe;
    int32_t is_recording;
    int32_t extra_mirrored;
    int32_t has_encoded_at;
    int32_t parameter_set_count;
    double encoded_at;
} OpcRelayFrameMeta;

/// `WatcherRelayControlOptions`, flattened. Counts are clamped to OPC_RELAY_OPTIONS_CAP.
/// `is_nano` is -1 when the host did not say.
typedef struct {
    int32_t is_recording;
    int32_t battery_percent;
    int32_t allows_control_requests;
    int32_t is_nano;
    int32_t has_control_options;
    int32_t iso_count;
    int32_t shutter_count;
    int32_t zoom_count;
    int32_t iso_indices[OPC_RELAY_OPTIONS_CAP];
    int32_t shutter_denominators[OPC_RELAY_OPTIONS_CAP];
    int32_t zoom_hundredths[OPC_RELAY_OPTIONS_CAP];
    char format[OPC_RELAY_TEXT_CAP];
    char color[OPC_RELAY_TEXT_CAP];
    char zoom[OPC_RELAY_TEXT_CAP];
    char live_fps[OPC_RELAY_TEXT_CAP];
    char camera_name[OPC_RELAY_TEXT_CAP];
    char iso[OPC_RELAY_TEXT_CAP];
    char shutter[OPC_RELAY_TEXT_CAP];
    char camera_model[OPC_RELAY_TEXT_CAP];
} OpcRelayState;

/// `WatcherRelayJoinDenied`. A passcode prompt is the only recoverable denial.
typedef struct {
    int32_t passcode_required;
    char reason[OPC_RELAY_REASON_CAP];
} OpcRelayJoinDenied;

/// `WatcherRelayControlToken`.
typedef struct {
    int32_t holder_is_recipient;
    char holder_name[OPC_RELAY_TEXT_CAP];
} OpcRelayControlToken;

/// `WatcherRelayHello` as returned by the host on an accepted join.
typedef struct {
    int32_t version;
    char host_name[OPC_RELAY_TEXT_CAP];
    char camera_name[OPC_RELAY_TEXT_CAP];
} OpcRelayHello;

/// Fitted-picture focus coordinates in thousandths, from `WatcherFocusPoint`.
typedef struct {
    int32_t x;
    int32_t y;
} OpcRelayFocusPoint;

/// Constants the host must not hardcode.
typedef struct {
    int32_t version;
    int32_t hevc_codec;
    int32_t max_payload_bytes;
    int32_t framing_header_bytes;
    int32_t max_retries;
    int32_t join_timeout_ms;
    int32_t silence_timeout_ms;
    int32_t reserved;
    char service_type[OPC_RELAY_TEXT_CAP];
    char txt_camera[8];
    char txt_watchable[8];
} OpcRelayProtocolInfo;

// ---- Camera link ----------------------------------------------------------
//
// Command kinds for `opc_camera_command`. Each maps to one builder in the core's
// `Commands`; the shell never assembles payload bytes itself. Integer and real
// arguments are passed positionally — see the Rust `Command` enum for the shapes.

#define OPC_CAM_SESSION_WAKE 0
#define OPC_CAM_SESSION_KEEPALIVE 1
#define OPC_CAM_GIMBAL_INIT 2
#define OPC_CAM_APP_PRESENCE 3
#define OPC_CAM_LIVE_VIEW_ENABLE 4
#define OPC_CAM_NANO_LIVE_GATE 5

#define OPC_CAM_RECORD_START 10
#define OPC_CAM_RECORD_STOP 11
#define OPC_CAM_SHOOT_PHOTO 12
#define OPC_CAM_SET_SHOOTING_MODE 13

#define OPC_CAM_ZOOM_FACTOR 20
#define OPC_CAM_ZOOM_LENS 21
#define OPC_CAM_ZOOM_SLEW 22
#define OPC_CAM_ZOOM_STOP 23

#define OPC_CAM_GIMBAL_RECENTER 30
#define OPC_CAM_GIMBAL_FLIP 31
#define OPC_CAM_GIMBAL_FOLLOW 32
#define OPC_CAM_GIMBAL_FPV 33
#define OPC_CAM_GIMBAL_STICK 34
#define OPC_CAM_GIMBAL_SPEED 35
#define OPC_CAM_GIMBAL_TIMED_STOP 36
#define OPC_CAM_GIMBAL_PARAMS_GET 37
#define OPC_CAM_GIMBAL_TILT_LOCK 38

#define OPC_CAM_TRACK_SET 40
#define OPC_CAM_TRACK_CLEAR 41
#define OPC_CAM_TRACK_POLL 42
#define OPC_CAM_FOCUS_TRACK_SET 43
#define OPC_CAM_FOCUS_TRACK_GET 44

#define OPC_CAM_SET_ISO_INDEX 50
#define OPC_CAM_SET_ISO_LIMIT 51
#define OPC_CAM_SET_SHUTTER 52
#define OPC_CAM_SET_EV 53
#define OPC_CAM_SET_WB_AUTO 54
#define OPC_CAM_SET_WB_CUSTOM 55
#define OPC_CAM_SET_COLOR_MODE 56
#define OPC_CAM_SET_FOCUS_MODE 57
#define OPC_CAM_SET_VIDEO_FORMAT 58
#define OPC_CAM_SET_FOV 59

#define OPC_CAM_PARAM_GET 60
#define OPC_CAM_GET_WIFI_SSID 61
#define OPC_CAM_GET_WIFI_PASSWORD 62
#define OPC_CAM_ENTER_PLAYBACK 63
#define OPC_CAM_EXIT_PLAYBACK 64

// Settings the desktop sheets write. Bytes are the core's own enum raw values.
#define OPC_CAM_SET_EXPO_MODE 65
#define OPC_CAM_SET_AUDIO_CHANNEL 66
#define OPC_CAM_SET_VOCAL_BOOST 67

// Media: the catalogue list, its trigger, delete, favourite, and the Pocket 3
// playback entry (`0x01/0x01`, two steps).
#define OPC_CAM_MEDIA_LIST 68
#define OPC_CAM_MEDIA_LIST_TRIGGER 69
#define OPC_CAM_MEDIA_DELETE 70
#define OPC_CAM_MEDIA_FAVORITE 71
#define OPC_CAM_PLAYBACK_SPECIAL 72
// Native timed gimbal target `0x04/0x14`: yaw and native pitch in 0.1°, duration
// in tenths of a second (1–255). The core refuses an unreachable or ill-timed one.
#define OPC_CAM_GIMBAL_TIMED_TARGET 73
/* Mimo's tap-to-focus burst, one frame each: 0x22 spot, 0x30 region, 0x68 hint, 0x32 commit. */
#define OPC_CAM_TAP_FOCUS_PREPARE 74
#define OPC_CAM_TAP_FOCUS_POINT 75
#define OPC_CAM_TAP_FOCUS_HINT 76
#define OPC_CAM_TAP_FOCUS_COMMIT 77

/* False-colour scales for opc_false_color_cube / opc_false_color_legend. */
#define OPC_FALSE_COLOR_STOPS 0
#define OPC_FALSE_COLOR_IRE 1
#define OPC_FALSE_COLOR_LIMITS 2
#define OPC_FALSE_COLOR_EL_ZONE 3

/* CameraSetMailbox decisions, for opc_mailbox_*. */
#define OPC_MAILBOX_LAUNCH 0
#define OPC_MAILBOX_COALESCE 1
#define OPC_MAILBOX_ACK_ACCEPT 0
#define OPC_MAILBOX_ACK_ACCEPT_LATE 1
#define OPC_MAILBOX_ACK_DROP_SUPERSEDED 2
#define OPC_MAILBOX_ACK_DROP_UNKNOWN 3
#define OPC_MAILBOX_TIMEOUT_SUBSCRIBE_MATCHES 0
#define OPC_MAILBOX_TIMEOUT_WAIT_LATE 1
#define OPC_MAILBOX_TIMEOUT_LAUNCH_PENDING 2
#define OPC_MAILBOX_TIMEOUT_IDLE 3
#define OPC_MAILBOX_PENDING_IMMEDIATE 0
#define OPC_MAILBOX_PENDING_AFTER_HOLD 1
#define OPC_MAILBOX_PENDING_NONE 2

/* A 0x02/0xA5 tracking poll reply, for opc_tracking_poll. */
#define OPC_TRACKING_UNKNOWN (-1)
#define OPC_TRACKING_IDLE 0
#define OPC_TRACKING_LOCKED 1
#define OPC_TRACKING_LOCKED_BOX 2

/* What a zoom write needs first, for opc_zoom_hop. */
#define OPC_ZOOM_HOP_NONE 0
#define OPC_ZOOM_HOP_COLOR 1
#define OPC_ZOOM_HOP_BLOCKED 2

// `DumlTransport.PktType`.
#define OPC_PKT_HANDSHAKE 0x00
#define OPC_PKT_TELEMETRY 0x01
#define OPC_PKT_VIDEO 0x02
#define OPC_PKT_ACKED_DATA 0x03
#define OPC_PKT_WINDOW_ACK 0x04
#define OPC_PKT_COMMAND 0x05

/// The three window cursors a pktType-0x04 ACK carries. Group 0 is video, group 1 is
/// acked data (command replies), group 2 is seeded from telemetry. Telemetry must never
/// rewind group 0 after the first video packet, nor group 1 after the first reply —
/// either mistake stops the picture while the HUD stays live.
typedef struct {
    uint32_t video;
    uint32_t acked_data;
    uint32_t extra;
    int32_t has_acked_data;
    int32_t has_extra;
} OpcAckWindows;

/// One DUML frame, unpacked. Payload bytes are carried separately.
typedef struct {
    uint8_t sender;
    uint8_t receiver;
    uint8_t flags;
    uint8_t cmd_set;
    uint8_t cmd_id;
    uint8_t reserved[1];
    uint16_t seq;
    uint32_t payload_offset;
    uint32_t payload_len;
} OpcDumlFrame;

// ---- Feed watchdog --------------------------------------------------------
//
// What the shell knows about the feed right now. Ages are seconds, and a **negative**
// age means "never seen" — zero is a real age. Doubles lead so the record has no
// padding surprises across the boundary.

/// `FeedWatchdog.Action`: the recover ladder, in order of escalation.
#define OPC_WATCHDOG_NONE 0
#define OPC_WATCHDOG_RESEND_ENABLE 1
#define OPC_WATCHDOG_REBUILD_DECODER 2
#define OPC_WATCHDOG_REOPEN_DATALINK 3
#define OPC_WATCHDOG_FULL_REJOIN 4

typedef struct {
    double now;
    double last_decoded_frame_age;
    double last_video_packet_age;
    double last_access_unit_age;
    double last_status_age;
    double last_ble_notify_age;
    double seconds_since_last_rebuild;
    double seconds_since_last_enable;
    double seconds_since_focus_track_set;
    double seconds_since_zoom_set;
    double seconds_since_gimbal_throw;
    double seconds_since_camera_set;
    int32_t flow_healthy;
    int32_t path_ready;
    int32_t has_format;
    int32_t decoder_failed;
    int32_t live;
    int32_t saw_picture;
    int32_t tcp_poke_ready;
    int32_t displayed_image_removed;
    int32_t had_video;
    int32_t reserved;
} OpcWatchdogSnapshot;

// ---- Camera status --------------------------------------------------------
//
// What the HUD shows. A flat record rather than the core's full `CameraStatus`: the
// desktop shell reads what it draws, and the rest stays where it belongs.
//
// `-1` means the camera has not said. Fields that can legitimately be negative carry a
// separate `has_` flag instead.

#define OPC_STATUS_LIST_CAP 32

typedef struct {
    int32_t battery_percent;
    int32_t charging;
    int32_t docked;
    int32_t is_recording;
    int32_t in_playback;
    int32_t record_elapsed_sec;
    int32_t record_remaining_sec;
    int32_t shooting_mode;
    int32_t iso;
    int32_t iso_index;
    int32_t iso_limit;
    /// Third-stops from zero; negative is a real value, so read `has_ev` first.
    int32_t ev_thirds;
    int32_t has_ev;
    int32_t shutter_denom;
    int32_t fps;
    int32_t video_resolution;
    int32_t video_frame_rate;
    int32_t color_mode;
    int32_t expo_mode;
    int32_t white_balance_kelvin;
    /// Tint can be negative, so read `has_white_balance_tint` first.
    int32_t white_balance_tint;
    int32_t has_white_balance_tint;
    int32_t focus_mode;
    int32_t focus_track;
    int32_t storage_free_mb;
    int32_t storage_total_mb;
    /// Zoom in hundredths: 250 is 2.5x.
    int32_t zoom_hundredths;
    int32_t available_shutter_count;
    int32_t available_iso_count;
    int32_t available_format_count;
    int32_t available_color_count;
    int32_t reserved;
    // Gimbal attitude from the `0x04/0x05` heartbeat, 0.1°: yaw i16 @4, display
    // tilt (look-up positive) from @20, and the native absolute pitch i16 @0 that
    // `0x04/0x14` targets take. `gimbal_attitude_seq` counts pushes; zero is none.
    int32_t gimbal_yaw_tenth;
    int32_t gimbal_pitch_tenth;
    int32_t gimbal_native_pitch_tenth;
    int32_t gimbal_attitude_seq;
    /// The values this body offers, as it reported them. A picker that invents its own
    /// list offers settings the camera will refuse.
    int32_t available_shutter[OPC_STATUS_LIST_CAP];
    int32_t available_iso[OPC_STATUS_LIST_CAP];
    int32_t available_format_resolution[OPC_STATUS_LIST_CAP];
    int32_t available_format_frame_rate[OPC_STATUS_LIST_CAP];
    int32_t available_color[OPC_STATUS_LIST_CAP];
} OpcCameraStatus;

#endif
