//! End-to-end checks across the C ABI.
//!
//! These compile only when the Swift core is linked (`just desktop-core`). Everything
//! here goes through the real facade, so a change to the core's framing, JSON shape, or
//! retry ladder is caught on the Rust side too.

#![cfg(opc_core_linked)]

use opc_core_sys as sys;
use opc_relay::ffi::{self, Command, ProtocolInfo, RecoveryAction, RelayError, WatcherPolicy};

fn info() -> ProtocolInfo {
    ProtocolInfo::load().expect("the core should publish its relay contract")
}

#[test]
fn the_core_publishes_the_relay_contract() {
    let info = info();
    assert_eq!(info.version, 1);
    assert_eq!(info.service_type, "_opc-mon._tcp");
    assert_eq!(info.mdns_service_type(), "_opc-mon._tcp.local.");
    assert_eq!(info.framing_header_bytes, 5);
    assert_eq!(info.txt_camera, "c");
    assert_eq!(info.txt_watchable, "w");
    assert_eq!(info.max_retries, 3);
}

#[test]
fn a_framed_message_round_trips() {
    let wire = ffi::encode_message(sys::OPC_RELAY_KIND_STATE, &[1, 2, 3, 4]).unwrap();
    assert_eq!(wire.len(), 9);
    let message = ffi::decode_message(&wire).unwrap().expect("one message");
    assert_eq!(message.kind, sys::OPC_RELAY_KIND_STATE);
    assert_eq!(message.consumed, 9);
    assert_eq!(&wire[message.payload], &[1, 2, 3, 4]);
}

#[test]
fn a_short_read_asks_for_more_bytes() {
    let wire = ffi::encode_message(sys::OPC_RELAY_KIND_STATE, &[1, 2, 3, 4]).unwrap();
    for truncated in 0..wire.len() {
        assert_eq!(ffi::decode_message(&wire[..truncated]).unwrap(), None);
    }
}

#[test]
fn an_unknown_kind_is_refused_rather_than_skipped() {
    assert_eq!(
        ffi::decode_message(&[0, 0, 0, 1, 0x7F]),
        Err(RelayError::UnknownKind)
    );
}

#[test]
fn two_messages_in_one_read_are_taken_in_order() {
    let mut stream = ffi::encode_message(sys::OPC_RELAY_KIND_STATE, b"first").unwrap();
    stream.extend(ffi::encode_message(sys::OPC_RELAY_KIND_CONTROL_TOKEN, b"second").unwrap());

    let first = ffi::decode_message(&stream).unwrap().unwrap();
    assert_eq!(&stream[first.payload.clone()], b"first");
    let rest = &stream[first.consumed..];
    let second = ffi::decode_message(rest).unwrap().unwrap();
    assert_eq!(second.kind, sys::OPC_RELAY_KIND_CONTROL_TOKEN);
    assert_eq!(&rest[second.payload], b"second");
}

#[test]
fn a_frame_payload_splits_into_metadata_and_an_access_unit() {
    let meta = br#"{"codec":1,"isKeyframe":true,"isRecording":false,"extraMirrored":false,"encodedAt":4.5}"#;
    let unit = [0u8, 0, 0, 1, 0x26, 0x01, 0x02];
    let mut payload = (meta.len() as u32).to_be_bytes().to_vec();
    payload.extend_from_slice(meta);
    payload.extend_from_slice(&unit);

    let blob = ffi::decode_frame_blob(&payload).unwrap();
    assert!(blob.meta.is_keyframe);
    assert!(!blob.meta.is_recording);
    assert_eq!(blob.meta.encoded_at, Some(4.5));
    assert_eq!(&payload[blob.access_unit], &unit);
    assert_eq!(&payload[blob.meta_json], meta.as_slice());
}

#[test]
fn keyframe_parameter_sets_come_back_in_order() {
    let meta = br#"{"codec":1,"isKeyframe":true,"isRecording":false,"extraMirrored":false,"parameterSets":["QAEM","QgEB"]}"#;
    let sets = ffi::parameter_sets(meta, 2).unwrap();
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[0], vec![0x40, 0x01, 0x0C]);
    assert_eq!(sets[1], vec![0x42, 0x01, 0x01]);
}

#[test]
fn host_state_is_mirrored_onto_the_watcher() {
    let json = br#"{"isRecording":true,"format":"4K 30p","color":"D-Log M","zoom":"1.0x",
        "liveFPS":"25","batteryPercent":73,"cameraName":"Osmo Pocket 4 Pro","iso":"400",
        "shutter":"1/50","allowsControlRequests":true,"isNano":false,
        "controlOptions":{"isoIndices":[100,200,400],"shutterDenominators":[50],
        "zoomHundredths":[100,200]}}"#;
    let state = ffi::decode_state(json).unwrap();
    assert!(state.is_recording);
    assert_eq!(state.battery_percent, 73);
    assert_eq!(state.camera_name, "Osmo Pocket 4 Pro");
    assert_eq!(state.iso_indices, vec![100, 200, 400]);
    assert_eq!(state.shutter_denominators, vec![50]);
    assert_eq!(state.is_nano, Some(false));
}

#[test]
fn an_unstated_nano_flag_stays_unknown() {
    let json = br#"{"isRecording":false,"format":"","color":"","zoom":"","liveFPS":"",
        "batteryPercent":-1,"cameraName":"","iso":"","shutter":"",
        "allowsControlRequests":true}"#;
    let state = ffi::decode_state(json).unwrap();
    assert_eq!(state.is_nano, None);
    assert!(state.iso_indices.is_empty());
}

#[test]
fn an_empty_passcode_is_sent_as_absent() {
    let payload = ffi::encode_hello("Studio PC", "", "desktop-1").unwrap();
    let text = String::from_utf8(payload).unwrap();
    assert!(text.contains("Studio PC"));
    assert!(text.contains("desktop-1"));
    assert!(!text.contains("passcode"));
}

#[test]
fn a_passcode_is_carried_when_the_host_asks_for_one() {
    let payload = ffi::encode_hello("Studio PC", "4242", "desktop-1").unwrap();
    let text = String::from_utf8(payload).unwrap();
    assert!(text.contains("4242"));
}

#[test]
fn commands_encode_through_the_core_enum() {
    for command in [
        Command::ToggleRecording,
        Command::SetIso(400),
        Command::SetZoom(250),
        Command::SetWhiteBalance {
            mode: 1,
            kelvin: 4200,
            tint: 20,
        },
        Command::TapFocus {
            camera_x: 500,
            camera_y: 250,
            coordinate_width: 1000,
            coordinate_height: 1000,
        },
    ] {
        assert!(!ffi::encode_command(command).unwrap().is_empty());
    }
}

#[test]
fn focus_is_fitted_to_the_picture_and_mirrors() {
    assert_eq!(
        ffi::map_focus(320.0, 180.0, 1280.0, 720.0, false),
        Ok((250, 250))
    );
    assert_eq!(
        ffi::map_focus(320.0, 180.0, 1280.0, 720.0, true),
        Ok((750, 250))
    );
    assert_eq!(
        ffi::map_focus(-1.0, 180.0, 1280.0, 720.0, false),
        Err(RelayError::OutOfRange)
    );
}

#[test]
fn the_retry_ladder_comes_from_the_core() {
    let mut policy = WatcherPolicy::new(0.0);
    policy.connected(0.0);
    assert_eq!(policy.tick(1.0), RecoveryAction::None);
    assert!(!policy.retry_pending());

    // Five seconds of silence is the core's deadline, then a one-second first rung.
    assert_eq!(policy.tick(6.0), RecoveryAction::None);
    assert!(policy.retry_pending());
    assert_eq!(policy.retry_count(), 1);
    assert_eq!(policy.tick(7.1), RecoveryAction::Reconnect);
}

#[test]
fn three_failed_attempts_exhaust_the_ladder() {
    let mut policy = WatcherPolicy::new(0.0);
    policy.connected(0.0);
    let mut now = 0.0;
    for attempt in 1..=3 {
        assert_eq!(policy.disconnected(now), RecoveryAction::None);
        assert_eq!(policy.retry_count(), attempt);
        now += 10.0;
        assert_eq!(policy.tick(now), RecoveryAction::Reconnect);
    }
    assert_eq!(policy.disconnected(now), RecoveryAction::Exhausted);
    assert_eq!(policy.tick(now + 100.0), RecoveryAction::None);
}

#[test]
fn delivery_delay_growth_is_measured_without_a_shared_clock() {
    let mut policy = WatcherPolicy::new(0.0);
    policy.connected(0.0);
    assert!(!policy.is_falling_behind(Some(0.0), 1.0));
    assert!(!policy.is_falling_behind(Some(1.0), 1.5));
    assert!(policy.is_falling_behind(Some(2.0), 4.0));
}

#[test]
fn a_sender_without_a_clock_never_trips_the_guard() {
    let mut policy = WatcherPolicy::new(0.0);
    policy.connected(0.0);
    assert!(!policy.is_falling_behind(None, 100.0));
}
