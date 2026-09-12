//! The UDP datalink.
//!
//! Owns the socket and the clock; borrows every decision. [`Sequencer`] says what is
//! due, [`AckPump`] says what an acknowledgement carries, and the core says what the
//! bytes are. The one thing this file decides on its own is the sequence bookkeeping,
//! which is transcribed from the iOS driver: the DUML frame sequence advances by one per
//! command, the transport sequence by eight, and the command counter by one.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::depacketizer::Depacketizer;
use crate::health::FeedHealth;
use crate::packed::DumlFrame;
use crate::sequence::{Outgoing, Phase, Sequencer};
use crate::status::{Status, StatusDecoder};
use crate::transport::{self, AckPump, PktType};
use crate::watchdog::{Recovery, Watchdog};
use crate::{softap, CameraError, Command};

/// How long a read may block before the caller gets a turn to tick the clock. Short
/// enough that a 40 Hz pump keeps its cadence.
const READ_TIMEOUT: Duration = Duration::from_millis(5);
const READ_BUFFER: usize = 4096;

/// What happened while polling.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// The camera answered the session open.
    Opened,
    /// A complete HEVC access unit.
    Picture(Vec<u8>),
    /// A command reply or a piece of telemetry.
    Frame(DumlFrame),
    /// The camera said something the HUD shows.
    StatusChanged,
    /// The camera never answered the handshake.
    Unreachable,
    /// The feed stalled and the watchdog acted. `RebuildDecoder` and `FullRejoin` are
    /// the shell's to carry out; the other two are handled here.
    Recovering(Recovery),
}

#[derive(Debug)]
pub enum SessionError {
    Io(io::Error),
    Camera(CameraError),
    /// The local port chosen is one the datalink must not bind.
    ForbiddenLocalPort(u16),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Camera(error) => write!(f, "{error}"),
            Self::ForbiddenLocalPort(port) => write!(
                f,
                "local port {port} must not be bound — the camera's own port accepts \
                 telemetry and drops every video packet"
            ),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<io::Error> for SessionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<CameraError> for SessionError {
    fn from(error: CameraError) -> Self {
        Self::Camera(error)
    }
}

/// One camera datalink.
#[derive(Debug)]
pub struct CameraSession {
    socket: UdpSocket,
    remote: SocketAddr,
    session_id: u16,
    base_seq: u16,
    /// The DUML frame sequence. One per command.
    duml_seq: u16,
    /// The transport sequence. Eight per command.
    udp_seq: u16,
    command_counter: u8,
    sequencer: Sequencer,
    pump: AckPump,
    depacketizer: Depacketizer,
    health: FeedHealth,
    watchdog: Watchdog,
    status: StatusDecoder,
    started: Instant,
    buffer: Vec<u8>,
}

impl CameraSession {
    /// Opens a datalink to the camera at its usual address.
    ///
    /// `session_id` and `base_seq` should be fresh per connect; a fixed base sequence
    /// can wedge the camera.
    pub fn connect(session_id: u16, base_seq: u16) -> Result<Self, SessionError> {
        let host: Ipv4Addr = softap::host()
            .parse()
            .unwrap_or(Ipv4Addr::new(192, 168, 2, 1));
        let remote = SocketAddr::new(IpAddr::V4(host), softap::remote_port());
        Self::connect_to(remote, session_id, base_seq)
    }

    /// Opens a datalink to an explicit address. The tests point this at a fake camera.
    pub fn connect_to(
        remote: SocketAddr,
        session_id: u16,
        base_seq: u16,
    ) -> Result<Self, SessionError> {
        // Bind an ephemeral local port, always. The camera's own port is the remote only.
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let local = socket.local_addr()?.port();
        if !softap::may_bind_local_port(0) {
            return Err(SessionError::ForbiddenLocalPort(local));
        }
        socket.set_read_timeout(Some(READ_TIMEOUT))?;
        socket.connect(remote)?;

        let started = Instant::now();
        Ok(Self {
            socket,
            remote,
            session_id,
            base_seq,
            duml_seq: 0,
            udp_seq: base_seq,
            command_counter: 0,
            sequencer: Sequencer::new(0.0),
            pump: AckPump::new(base_seq),
            depacketizer: Depacketizer::new(),
            health: FeedHealth::new(),
            watchdog: Watchdog::new(),
            status: StatusDecoder::new(None),
            started,
            buffer: vec![0; READ_BUFFER],
        })
    }

    /// The port this datalink is actually sending from.
    pub fn local_port(&self) -> u16 {
        self.socket
            .local_addr()
            .map(|address| address.port())
            .unwrap_or(0)
    }

    pub fn remote(&self) -> SocketAddr {
        self.remote
    }

    pub fn phase(&self) -> Phase {
        self.sequencer.phase()
    }

    /// How many access units were abandoned incomplete.
    pub fn dropped_access_units(&self) -> i32 {
        self.depacketizer.dropped()
    }

    /// What the camera last said about itself.
    pub fn status(&self) -> Status {
        self.status.status()
    }

    /// Tells the status decoder which body this is, so it reads the model-specific
    /// encodings — colour modes differ between a Pocket 4, a Pocket 3 and a Nano.
    pub fn set_model(&mut self, model_id: i32) {
        self.status = StatusDecoder::new(Some(model_id));
    }

    /// Which rung of the recover ladder the watchdog is resting on.
    pub fn recovery_stage(&self) -> String {
        self.watchdog.stage()
    }

    /// Tells the session whether the machine is still on the camera's network. A socket
    /// that is off-path looks exactly like a camera that stopped answering.
    pub fn set_path_ready(&mut self, ready: bool) {
        self.health.set_path_ready(ready);
    }

    /// The shell reports whether its decoder is wedged; the watchdog escalates on it.
    pub fn set_decoder_failed(&mut self, failed: bool) {
        self.health.set_decoder_failed(failed);
    }

    /// A picture actually reached the screen, which is not the same as one arriving.
    pub fn note_presented(&mut self) {
        let now = self.now();
        self.health.note_decoded_frame(now);
    }

    /// Queues an operator command for the next tick.
    pub fn send(&mut self, command: Command) {
        self.sequencer.enqueue(command);
    }

    /// Monotonic seconds since the datalink opened.
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Reads what has arrived, then sends what is due.
    ///
    /// Call this in a loop. It blocks for at most the read timeout, so a caller that
    /// does nothing else still keeps the pump on cadence.
    pub fn poll(&mut self) -> Result<Vec<SessionEvent>, SessionError> {
        let mut events = Vec::new();
        self.drain(&mut events)?;

        let now = self.now();
        for due in self.sequencer.tick(now) {
            self.dispatch(due)?;
        }
        if self.sequencer.phase() == Phase::Unreachable {
            events.push(SessionEvent::Unreachable);
        }
        self.recover(now, &mut events)?;
        Ok(events)
    }

    /// Asks the watchdog whether the feed has stalled, and acts on its answer.
    fn recover(&mut self, now: f64, events: &mut Vec<SessionEvent>) -> Result<(), SessionError> {
        let live = matches!(self.sequencer.phase(), Phase::Waiting | Phase::Live);
        let snapshot = self.health.snapshot(now, live);
        let Some(action) = self.watchdog.tick(&snapshot) else {
            return Ok(());
        };

        match action {
            Recovery::ResendEnable => {
                // Deliberately not through the sequencer: that one enables once per
                // session by design, and repeats belong here.
                let datagram = self.command_datagram(Command::LiveViewEnable)?;
                self.socket.send(&datagram)?;
                self.health.note_enable(now);
            }
            Recovery::ReopenDatalink => {
                self.depacketizer.reset();
                self.sequencer.restart(now);
                self.health.note_datalink_rebuilt(now);
            }
            // A wedged decoder and a full rejoin are the shell's to carry out: one owns
            // the decoder, the other owns Bluetooth.
            Recovery::RebuildDecoder | Recovery::FullRejoin => {}
        }
        events.push(SessionEvent::Recovering(action));
        Ok(())
    }

    fn drain(&mut self, events: &mut Vec<SessionEvent>) -> Result<(), SessionError> {
        loop {
            let mut scratch = std::mem::take(&mut self.buffer);
            let read = self.socket.recv(&mut scratch);
            let outcome = match read {
                Ok(count) => {
                    let datagram = scratch[..count].to_vec();
                    self.buffer = scratch;
                    Some(datagram)
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    self.buffer = scratch;
                    None
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    self.buffer = scratch;
                    continue;
                }
                Err(error) => {
                    self.buffer = scratch;
                    return Err(error.into());
                }
            };
            let Some(datagram) = outcome else {
                return Ok(());
            };
            self.receive(&datagram, events)?;
        }
    }

    fn receive(
        &mut self,
        datagram: &[u8],
        events: &mut Vec<SessionEvent>,
    ) -> Result<(), SessionError> {
        // Every datagram feeds the cursors, whatever else it means.
        self.pump.observe(datagram);

        let now = self.now();
        if transport::is_handshake(datagram) {
            let opening = self.sequencer.phase() == Phase::Handshaking;
            self.sequencer.note_handshake_reply(now);
            if opening {
                // Ask for the pushes the HUD needs before anything else is queued: the
                // camera only sends its available-value lists to a subscriber.
                self.send_subscriptions()?;
                events.push(SessionEvent::Opened);
            }
            return Ok(());
        }

        match PktType::of(datagram) {
            Some(PktType::Video) => {
                self.sequencer.note_picture();
                self.health.note_video_packet(now);
                // The whole datagram, header included: the core reads the packet type at
                // byte 6 and the fragment index at bytes 16 to 18, and the encoded body
                // only starts at byte 20.
                if let Some(unit) = self.depacketizer.feed(datagram) {
                    self.health.note_access_unit(now);
                    events.push(SessionEvent::Picture(unit));
                }
            }
            Some(PktType::Telemetry | PktType::AckedData | PktType::Command) => {
                self.health.note_status(now);
                let mut changed = false;
                for frame in transport::scan_frames(datagram)? {
                    // `0x00/0x99` carries a subscription push — timecode and the lists
                    // of values this body actually offers — and is read differently.
                    changed |= if (frame.cmd_set, frame.cmd_id) == (0x00, 0x99) {
                        self.status.apply_push(&frame.payload)
                    } else {
                        self.status.apply(&frame)
                    };
                    events.push(SessionEvent::Frame(frame));
                }
                if changed {
                    events.push(SessionEvent::StatusChanged);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Subscribes to the status streams the HUD reads.
    fn send_subscriptions(&mut self) -> Result<(), SessionError> {
        for (index, key) in subscribe_keys().into_iter().enumerate() {
            let payload = transport::subscribe(&key, index as u32 + 1, self.duml_seq)?;
            self.duml_seq = self.duml_seq.wrapping_add(1);
            self.command_counter = self.command_counter.wrapping_add(1);

            let routing = transport::routing_header(self.udp_seq, self.command_counter, false)?;
            let mut datagram = transport::transport_header(
                PktType::Command,
                routing.len() + payload.len(),
                self.session_id,
                self.udp_seq,
            )?;
            self.udp_seq = self.udp_seq.wrapping_add(8);
            datagram.extend_from_slice(&routing);
            datagram.extend_from_slice(&payload);
            self.socket.send(&datagram)?;
        }
        Ok(())
    }

    fn dispatch(&mut self, due: Outgoing) -> Result<(), SessionError> {
        let datagram = match due {
            Outgoing::Handshake => {
                transport::handshake(self.session_id, self.udp_seq, self.base_seq)?
            }
            Outgoing::Ack => self.pump.datagram(self.session_id)?,
            Outgoing::EnableLiveView => {
                let now = self.now();
                self.health.note_enable(now);
                self.command_datagram(Command::LiveViewEnable)?
            }
            Outgoing::Command(command) => {
                let now = self.now();
                match command {
                    Command::ZoomFactor(_) | Command::ZoomLens(_) | Command::ZoomSlew(_) => {
                        self.health.note_zoom(now);
                    }
                    Command::FocusTrackSet(_) => self.health.note_focus_track(now),
                    Command::GimbalStick { .. } => self.health.note_gimbal_throw(now),
                    _ => self.health.note_camera_set(now),
                }
                self.command_datagram(command)?
            }
        };
        self.socket.send(&datagram)?;
        Ok(())
    }

    /// Wraps a command in its routing and transport headers, advancing all three
    /// counters the way the iOS driver does.
    fn command_datagram(&mut self, command: Command) -> Result<Vec<u8>, SessionError> {
        let frame = command.encode(self.duml_seq)?;
        self.duml_seq = self.duml_seq.wrapping_add(1);
        self.command_counter = self.command_counter.wrapping_add(1);

        let routing = transport::routing_header(self.udp_seq, self.command_counter, false)?;
        let mut datagram = transport::transport_header(
            PktType::Command,
            routing.len() + frame.len(),
            self.session_id,
            self.udp_seq,
        )?;
        self.udp_seq = self.udp_seq.wrapping_add(8);

        datagram.extend_from_slice(&routing);
        datagram.extend_from_slice(&frame);
        Ok(datagram)
    }
}

/// The subscription names the core says the HUD needs.
fn subscribe_keys() -> Vec<String> {
    // Safety: probing with a null destination only reports the size.
    let needed = unsafe { opc_core_sys::opc_status_subscribe_keys(std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let mut bytes = vec![0u8; needed as usize];
    // Safety: `bytes` has exactly the capacity the core asked for.
    let written =
        unsafe { opc_core_sys::opc_status_subscribe_keys(bytes.as_mut_ptr(), bytes.len()) };
    if written != needed {
        return Vec::new();
    }
    String::from_utf8_lossy(&bytes)
        .split('\n')
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .collect()
}
