//! Commands and acknowledgements, checked against the bytes the protocol notes specify.
//!
//! Compiles only with the Swift core linked (`just desktop-core`). Each command is
//! encoded and then scanned back out, so both directions of the facade are covered and
//! the opcode and payload are asserted against what `docs/` says the camera expects —
//! not merely against whatever the encoder happened to produce.

#![cfg(opc_core_linked)]

use opc_camera::{
    handshake, is_handshake, scan_frames, tap_focus, transport_header, transport_seq, AckPump,
    CameraError, Command, PktType,
};

/// Encodes a command and reads the single frame back out of it.
fn frame_of(command: Command, seq: u16) -> opc_camera::DumlFrame {
    let encoded = command
        .encode(seq)
        .expect("the core should build this command");
    let frames = scan_frames(&encoded).expect("the encoding should scan back");
    assert_eq!(frames.len(), 1, "one command is one frame");
    frames.into_iter().next().expect("a frame")
}

#[test]
fn recording_uses_the_documented_opcode_and_payload() {
    let start = frame_of(Command::RecordStart, 1);
    assert_eq!((start.cmd_set, start.cmd_id), (0x02, 0x02));
    assert_eq!(start.payload, vec![0x01]);

    let stop = frame_of(Command::RecordStop, 2);
    assert_eq!((stop.cmd_set, stop.cmd_id), (0x02, 0x02));
    assert_eq!(stop.payload, vec![0x00]);
}

#[test]
fn live_view_enable_is_the_one_pli() {
    let frame = frame_of(Command::LiveViewEnable, 3);
    assert_eq!((frame.cmd_set, frame.cmd_id), (0x09, 0xA8));
}

#[test]
fn zoom_stop_matches_the_notes() {
    let frame = frame_of(Command::ZoomStop, 4);
    assert_eq!((frame.cmd_set, frame.cmd_id), (0x02, 0xB8));
    assert_eq!(frame.payload, vec![0xFF, 0x00, 0x00, 0x00]);
}

#[test]
fn a_zoom_factor_becomes_a_lens_position() {
    let frame = frame_of(Command::ZoomFactor(2.0), 5);
    assert_eq!((frame.cmd_set, frame.cmd_id), (0x02, 0xB8));
    // `0x0A 0x4E` then the position, little endian.
    assert_eq!(frame.payload.len(), 4);
    assert_eq!(&frame.payload[..2], &[0x0A, 0x4E]);
}

#[test]
fn shutter_carries_the_denominator_with_the_high_bit_set() {
    let frame = frame_of(Command::SetShutter(50), 6);
    assert_eq!((frame.cmd_set, frame.cmd_id), (0x02, 0x28));
    let encoded = 50u16 | 0x8000;
    assert_eq!(
        frame.payload,
        vec![
            0x01,
            (encoded & 0xFF) as u8,
            (encoded >> 8) as u8,
            0x00,
            0x00,
            0x00,
            0x40
        ]
    );
}

#[test]
fn video_format_sends_resolution_then_frame_rate() {
    // 4K at 25p: the core's own raw values.
    let frame = frame_of(
        Command::SetVideoFormat {
            resolution: 0x10,
            frame_rate: 0x02,
        },
        7,
    );
    assert_eq!((frame.cmd_set, frame.cmd_id), (0x02, 0x18));
    assert_eq!(frame.payload, vec![0x10, 0x02, 0x00, 0x00, 0x00]);
}

#[test]
fn tracking_set_and_clear_use_the_same_opcode() {
    let set = frame_of(
        Command::TrackSet {
            id: 1,
            x: 0.25,
            y: 0.5,
            width: 0.2,
            height: 0.3,
        },
        8,
    );
    assert_eq!((set.cmd_set, set.cmd_id), (0x02, 0xA6));
    // Three lead bytes, the id, then four little-endian floats.
    assert_eq!(set.payload.len(), 3 + 2 + 16);

    let clear = frame_of(Command::TrackClear, 9);
    assert_eq!((clear.cmd_set, clear.cmd_id), (0x02, 0xA6));
    assert_eq!(clear.payload, vec![0u8; 21]);
}

#[test]
fn tracking_poll_is_a_short_get() {
    let frame = frame_of(Command::TrackPoll, 10);
    assert_eq!((frame.cmd_set, frame.cmd_id), (0x02, 0xA5));
    assert_eq!(frame.payload, vec![0x00]);
}

#[test]
fn the_gimbal_commands_all_build() {
    for command in [
        Command::GimbalRecenter,
        Command::GimbalFlip,
        Command::GimbalFollow,
        Command::GimbalFpv,
        Command::GimbalStick {
            axis0: 1024,
            axis1: 1024,
        },
        Command::GimbalTimedStop,
        Command::GimbalParamsGet,
    ] {
        let encoded = command
            .encode(11)
            .unwrap_or_else(|_| panic!("{command:?} should build"));
        assert!(!encoded.is_empty());
    }
}

#[test]
fn exposure_commands_build_at_their_own_opcodes() {
    assert_eq!(frame_of(Command::SetIsoIndex(0x03), 12).cmd_id, 0x2A);
    assert_eq!(frame_of(Command::SetFocusMode(0x02), 13).cmd_id, 0x24);
    assert_eq!(
        frame_of(Command::SetWhiteBalanceAuto { tint: 20 }, 14).cmd_id,
        0x2C
    );
    assert_eq!(
        frame_of(
            Command::SetWhiteBalanceCustom {
                kelvin: 4200,
                tint: 20
            },
            15
        )
        .cmd_id,
        0x2C
    );
}

#[test]
fn an_argument_the_core_refuses_comes_back_as_rejected() {
    // 0xEE is not on the ISO ladder.
    assert!(matches!(
        Command::SetIsoIndex(0xEE).encode(1),
        Err(CameraError::Rejected(_))
    ));
}

#[test]
fn tap_focus_is_three_frames_in_order() {
    let frames = tap_focus(0.5, 0.5, 20).expect("tap focus should build");
    assert_eq!(frames.len(), 3);
    assert!(frames.iter().all(|frame| !frame.is_empty()));
}

#[test]
fn a_handshake_identifies_itself() {
    let datagram = handshake(0x1234, 0, 0x0100).expect("a handshake");
    assert_eq!(datagram.len(), 48);
    assert!(is_handshake(&datagram));
    assert_eq!(PktType::of(&datagram), Some(PktType::Handshake));
    // The base sequence leads the payload.
    assert_eq!(&datagram[8..10], &[0x00, 0x01]);
}

/// An 8-byte header of the given type and sequence, with no payload.
fn datagram(kind: PktType, seq: u16) -> Vec<u8> {
    transport_header(kind, 0, 0x1234, seq).expect("a header")
}

/// 34-byte telemetry: the window cursors sit at bytes 10, 18 and 26.
fn telemetry(video: u16, acked: u16, extra: u16) -> Vec<u8> {
    let mut out = transport_header(PktType::Telemetry, 26, 0x1234, 7).expect("a header");
    out.resize(34, 0);
    out[10..12].copy_from_slice(&video.to_le_bytes());
    out[18..20].copy_from_slice(&acked.to_le_bytes());
    out[26..28].copy_from_slice(&extra.to_le_bytes());
    out
}

/// The three cursors out of a pktType-0x04 payload.
fn cursors(ack: &[u8]) -> (u16, u16, u16) {
    let payload = &ack[8..];
    let at = |group: usize| u16::from_le_bytes([payload[group * 8], payload[group * 8 + 1]]);
    (at(0), at(1), at(2))
}

#[test]
fn a_fresh_pump_repeats_the_handshake_base_in_every_group() {
    let pump = AckPump::new(0x0100);
    let ack = pump.datagram(0x1234).expect("an ack");
    // Eight header bytes plus three eight-byte groups and a two-byte tail.
    assert_eq!(ack.len(), 8 + 26);
    assert_eq!(cursors(&ack), (0x0100, 0x0100, 0x0100));
    assert!(!pump.saw_video());
}

#[test]
fn a_video_packet_moves_group_zero() {
    let mut pump = AckPump::new(0x0100);
    pump.observe(&datagram(PktType::Video, 0x0200));
    assert!(pump.saw_video());
    assert_eq!(cursors(&pump.datagram(0x1234).unwrap()).0, 0x0200);
}

#[test]
fn telemetry_seeds_group_zero_only_before_the_first_picture() {
    let mut pump = AckPump::new(0x0100);
    // Before any video, telemetry is allowed to seed the cursor.
    pump.observe(&telemetry(0x0180, 0x0190, 0x01A0));
    assert_eq!(cursors(&pump.datagram(0x1234).unwrap()).0, 0x0180);
    assert!(!pump.saw_video());

    // After a real picture, telemetry must never pull it back — this is the mistake
    // that closes HEVC while the HUD stays live.
    pump.observe(&datagram(PktType::Video, 0x0300));
    pump.observe(&telemetry(0x0180, 0x0190, 0x01A0));
    assert_eq!(cursors(&pump.datagram(0x1234).unwrap()).0, 0x0300);
    assert!(pump.saw_video());
}

#[test]
fn a_command_reply_moves_group_one_and_telemetry_cannot_rewind_it() {
    let mut pump = AckPump::new(0x0100);
    pump.observe(&datagram(PktType::AckedData, 0x0400));
    assert_eq!(cursors(&pump.datagram(0x1234).unwrap()).1, 0x0400);

    pump.observe(&telemetry(0x0180, 0x0190, 0x01A0));
    assert_eq!(
        cursors(&pump.datagram(0x1234).unwrap()).1,
        0x0400,
        "telemetry rewinding group 1 is what mutes SET and GET"
    );
}

#[test]
fn group_two_keeps_following_telemetry() {
    let mut pump = AckPump::new(0x0100);
    pump.observe(&telemetry(0x0180, 0x0190, 0x01A0));
    assert_eq!(cursors(&pump.datagram(0x1234).unwrap()).2, 0x01A0);
    pump.observe(&telemetry(0x0181, 0x0191, 0x01B0));
    assert_eq!(cursors(&pump.datagram(0x1234).unwrap()).2, 0x01B0);
}

#[test]
fn zero_is_a_real_cursor_not_a_missing_one() {
    let mut pump = AckPump::new(0x0100);
    pump.observe(&datagram(PktType::AckedData, 0));
    let windows = pump.windows();
    assert!(windows.saw_acked_data);
    assert_eq!(
        cursors(&pump.datagram(0x1234).unwrap()).1,
        0,
        "a seen cursor of zero must not fall back to the handshake base"
    );
}

#[test]
fn a_transport_sequence_reads_back() {
    assert_eq!(
        transport_seq(&datagram(PktType::Video, 0x0500)),
        Some(0x0500)
    );
}
