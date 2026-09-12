//! The datalink, run against a camera that is not there.
//!
//! A fake camera on loopback answers the handshake, sends telemetry and video, and
//! replies to commands, while recording everything the session sent it. That makes the
//! failure this port is most likely to hit — a session that connects, shows telemetry,
//! and never shows a picture — something a test can catch rather than something an
//! operator discovers on location.
//!
//! Compiles only with the Swift core linked (`just desktop-core`).

#![cfg(opc_core_linked)]

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opc_camera::{scan_frames, transport_header, CameraSession, Command, PktType, SessionEvent};

/// What the fake camera saw.
#[derive(Debug, Default)]
struct Seen {
    handshakes: usize,
    acks: usize,
    /// `(cmd_set, cmd_id)` of every command frame, in order.
    commands: Vec<(u8, u8)>,
}

struct FakeCamera {
    address: SocketAddr,
    seen: Arc<Mutex<Seen>>,
    stop: Arc<AtomicBool>,
    /// Set to freeze the feed while the session stays connected — the failure that looks
    /// like nothing at all from a log.
    mute_video: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FakeCamera {
    /// `video_after` is how many acknowledgements to wait before starting to send
    /// pictures, so a test can watch the pump run before the first frame.
    fn start(video_after: usize) -> Self {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a socket");
        socket
            .set_read_timeout(Some(Duration::from_millis(5)))
            .expect("a read timeout");
        let address = socket.local_addr().expect("an address");
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let mute_video = Arc::new(AtomicBool::new(false));

        let thread_seen = Arc::clone(&seen);
        let thread_stop = Arc::clone(&stop);
        let thread_mute = Arc::clone(&mute_video);
        let handle = std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            let mut peer: Option<SocketAddr> = None;
            let mut frame_number = 0u8;

            while !thread_stop.load(Ordering::Relaxed) {
                let Ok((count, from)) = socket.recv_from(&mut buffer) else {
                    continue;
                };
                peer = Some(from);
                let datagram = &buffer[..count];
                let mut seen = thread_seen.lock().expect("the record");

                if opc_camera::is_handshake(datagram) {
                    seen.handshakes += 1;
                    drop(seen);
                    // Answer with a handshake of our own.
                    let reply = opc_camera::handshake(0x1234, 0, 0x0100).expect("a handshake");
                    let _ = socket.send_to(&reply, from);
                    continue;
                }

                match PktType::of(datagram) {
                    Some(PktType::WindowAck) => {
                        seen.acks += 1;
                        let acks_seen = seen.acks;
                        drop(seen);
                        if acks_seen > video_after && !thread_mute.load(Ordering::Relaxed) {
                            // One fragment per frame. The core reports a frame complete
                            // when the *next* frame starts, so this streams steadily.
                            frame_number = frame_number.wrapping_add(1);
                            let packet =
                                video_packet(frame_number, &[0x00, 0x00, 0x00, 0x01, 0x26]);
                            let _ = socket.send_to(&packet, from);
                        }
                    }
                    Some(PktType::Command) => {
                        for frame in scan_frames(datagram).unwrap_or_default() {
                            seen.commands.push((frame.cmd_set, frame.cmd_id));
                        }
                        drop(seen);
                        // Reply on the acked-data channel, as a real camera does.
                        let reply =
                            transport_header(PktType::AckedData, 0, 0x1234, 8).expect("a header");
                        let _ = socket.send_to(&reply, from);
                    }
                    _ => {}
                }
            }
            let _ = peer;
        });

        Self {
            address,
            seen,
            stop,
            mute_video,
            handle: Some(handle),
        }
    }

    /// Keeps answering everything but stops sending pictures.
    fn freeze(&self) {
        self.mute_video.store(true, Ordering::Relaxed);
    }

    fn seen(&self) -> Seen {
        let seen = self.seen.lock().expect("the record");
        Seen {
            handshakes: seen.handshakes,
            acks: seen.acks,
            commands: seen.commands.clone(),
        }
    }
}

impl Drop for FakeCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A pktType-0x02 datagram: 8-byte transport header, fragment bookkeeping at bytes 16
/// to 18, and the encoded body from byte 20.
fn video_packet(frame_number: u8, body: &[u8]) -> Vec<u8> {
    let mut packet =
        transport_header(PktType::Video, 12 + body.len(), 0x1234, 0).expect("a header");
    packet.resize(20, 0);
    packet[16] = frame_number;
    packet[17] = 0;
    packet[18] = 0;
    packet.extend_from_slice(body);
    packet
}

/// Polls until `wanted` says it has seen enough, or the deadline passes.
fn run_until<F>(session: &mut CameraSession, timeout: Duration, mut wanted: F) -> Vec<SessionEvent>
where
    F: FnMut(&[SessionEvent]) -> bool,
{
    let deadline = Instant::now() + timeout;
    let mut collected = Vec::new();
    while Instant::now() < deadline {
        collected.extend(session.poll().expect("polling should not fail"));
        if wanted(&collected) {
            break;
        }
    }
    collected
}

#[test]
fn the_datalink_never_binds_the_cameras_own_port() {
    let camera = FakeCamera::start(usize::MAX);
    let session = CameraSession::connect_to(camera.address, 0x1234, 0x0100).expect("a session");
    assert_ne!(
        session.local_port(),
        opc_camera::softap::remote_port(),
        "binding the camera's port accepts telemetry and drops every video packet"
    );
    assert_ne!(session.local_port(), 0);
}

#[test]
fn a_session_opens_and_then_pumps() {
    let camera = FakeCamera::start(usize::MAX);
    let mut session = CameraSession::connect_to(camera.address, 0x1234, 0x0100).expect("a session");

    let events = run_until(&mut session, Duration::from_secs(2), |events| {
        events.contains(&SessionEvent::Opened)
    });
    assert!(
        events.contains(&SessionEvent::Opened),
        "the camera should have answered"
    );

    // Let the pump run for a quarter second and check the cadence over the wire.
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        session.poll().expect("polling should not fail");
    }
    let seen = camera.seen();
    assert!(
        (6..=14).contains(&seen.acks),
        "about ten acknowledgements in a quarter second, saw {}",
        seen.acks
    );
}

#[test]
fn live_view_is_enabled_once_over_a_real_socket() {
    let camera = FakeCamera::start(usize::MAX);
    let mut session = CameraSession::connect_to(camera.address, 0x1234, 0x0100).expect("a session");

    let deadline = Instant::now() + Duration::from_millis(600);
    while Instant::now() < deadline {
        session.poll().expect("polling should not fail");
    }

    let enables = camera
        .seen()
        .commands
        .iter()
        .filter(|(set, id)| *set == 0x09 && *id == 0xA8)
        .count();
    assert_eq!(
        enables, 1,
        "live view must be enabled exactly once per session"
    );
}

#[test]
fn pictures_arrive_once_the_camera_starts_sending_them() {
    let camera = FakeCamera::start(2);
    let mut session = CameraSession::connect_to(camera.address, 0x1234, 0x0100).expect("a session");

    let events = run_until(&mut session, Duration::from_secs(3), |events| {
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::Picture(_)))
            .count()
            >= 2
    });
    let pictures: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Picture(unit) => Some(unit),
            _ => None,
        })
        .collect();
    assert!(
        pictures.len() >= 2,
        "expected pictures, saw {}",
        pictures.len()
    );
    assert!(pictures.iter().all(|unit| !unit.is_empty()));
    assert_eq!(session.phase(), opc_camera::Phase::Live);
}

#[test]
fn an_operator_command_reaches_the_camera() {
    let camera = FakeCamera::start(usize::MAX);
    let mut session = CameraSession::connect_to(camera.address, 0x1234, 0x0100).expect("a session");
    run_until(&mut session, Duration::from_secs(2), |events| {
        events.contains(&SessionEvent::Opened)
    });

    session.send(Command::RecordStart);
    session.send(Command::ZoomFactor(2.0));
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        session.poll().expect("polling should not fail");
    }

    let commands = camera.seen().commands;
    assert!(
        commands.contains(&(0x02, 0x02)),
        "record start should have arrived, saw {commands:?}"
    );
    assert!(
        commands.contains(&(0x02, 0xB8)),
        "zoom should have arrived, saw {commands:?}"
    );
}

#[test]
fn a_camera_that_never_answers_is_reported_rather_than_hung() {
    // Nothing is listening on this port.
    let dead = SocketAddr::from((Ipv4Addr::LOCALHOST, 1));
    let mut session = CameraSession::connect_to(dead, 0x1234, 0x0100).expect("a session");

    let events = run_until(&mut session, Duration::from_secs(15), |events| {
        events.contains(&SessionEvent::Unreachable)
    });
    assert!(
        events.contains(&SessionEvent::Unreachable),
        "the session should give up rather than wait forever"
    );
}

#[test]
fn the_handshake_is_repeated_until_the_camera_answers() {
    let camera = FakeCamera::start(usize::MAX);
    let mut session = CameraSession::connect_to(camera.address, 0x1234, 0x0100).expect("a session");
    run_until(&mut session, Duration::from_secs(2), |events| {
        events.contains(&SessionEvent::Opened)
    });
    // One open is enough; the fake answers the first one.
    assert!(camera.seen().handshakes >= 1);
}

#[test]
fn a_frozen_feed_is_noticed_and_recovered() {
    use opc_camera::Recovery;

    let camera = FakeCamera::start(2);
    let mut session = CameraSession::connect_to(camera.address, 0x1234, 0x0100).expect("a session");

    // Get a picture first: the watchdog treats a feed that never started differently
    // from one that stopped.
    run_until(&mut session, Duration::from_secs(3), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::Picture(_)))
    });

    let enables_before = camera
        .seen()
        .commands
        .iter()
        .filter(|(set, id)| *set == 0x09 && *id == 0xA8)
        .count();
    assert_eq!(enables_before, 1, "one enable on the connect path");

    // The camera keeps answering, and simply stops sending pictures.
    camera.freeze();
    let events = run_until(&mut session, Duration::from_secs(8), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::Recovering(_)))
    });

    let recoveries: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Recovering(action) => Some(*action),
            _ => None,
        })
        .collect();
    assert!(
        !recoveries.is_empty(),
        "a feed that stops should be noticed, not waited on forever"
    );
    assert_eq!(
        recoveries[0],
        Recovery::ResendEnable,
        "the ladder starts by asking for live view again"
    );

    let enables_after = camera
        .seen()
        .commands
        .iter()
        .filter(|(set, id)| *set == 0x09 && *id == 0xA8)
        .count();
    assert!(
        enables_after > enables_before,
        "the watchdog should have asked for the feed again"
    );
    assert!(!session.recovery_stage().is_empty());
}

#[test]
fn a_healthy_feed_is_left_alone() {
    let camera = FakeCamera::start(2);
    let mut session = CameraSession::connect_to(camera.address, 0x1234, 0x0100).expect("a session");

    let events = run_until(&mut session, Duration::from_secs(4), |events| {
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::Picture(_)))
            .count()
            >= 20
    });
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::Recovering(_))),
        "nothing should be recovered while pictures keep arriving"
    );
    // And still exactly one enable over the whole run.
    let enables = camera
        .seen()
        .commands
        .iter()
        .filter(|(set, id)| *set == 0x09 && *id == 0xA8)
        .count();
    assert_eq!(enables, 1);
}
