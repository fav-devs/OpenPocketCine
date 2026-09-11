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

#endif
