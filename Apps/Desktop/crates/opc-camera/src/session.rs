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

use crate::command;
use crate::depacketizer::Depacketizer;
use crate::health::FeedHealth;
use crate::mailbox::SetOutcome;
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
/// The camera ignores `0x09/0xa8` when it follows subscriptions in the same burst.
const SUBSCRIBE_SETTLE: f64 = 0.150;

/// What happened while polling.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// The camera answered the session open.
    Opened,
    /// A complete HEVC access unit.
    Picture(Vec<u8>),
    /// A command reply or a piece of telemetry.
    Frame(DumlFrame),
    /// What became of a live-control SET.
    Set(SetOutcome),
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
    /// A pending enable after the subscription writes have reached the camera.
    enable_not_before: Option<f64>,
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
        let socket = Self::open_socket(remote)?;

        let started = Instant::now();
        Ok(Self {
            socket,
            remote,
            session_id,
            base_seq,
            duml_seq: 0,
            udp_seq: base_seq,
            command_counter: 0,
            sequencer: new_sequencer(0.0),
            pump: AckPump::new(base_seq),
            depacketizer: Depacketizer::new(),
            health: FeedHealth::new(),
            watchdog: Watchdog::new(),
            enable_not_before: None,
            status: StatusDecoder::new(None),
            started,
            buffer: vec![0; READ_BUFFER],
        })
    }

    /// Opens the camera flow on a new ephemeral client port. The camera's :9004 is
    /// remote-only; keeping a wedged Windows socket through recovery leaves video on
    /// the old five-tuple even while telemetry appears healthy.
    fn open_socket(remote: SocketAddr) -> Result<UdpSocket, SessionError> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let local = socket.local_addr()?.port();
        if !softap::may_bind_local_port(0) {
            return Err(SessionError::ForbiddenLocalPort(local));
        }
        socket.set_read_timeout(Some(READ_TIMEOUT))?;
        socket.connect(remote)?;
        Ok(socket)
    }

    /// Recreates the local half of the UDP flow while Wi-Fi remains associated.
    fn reopen_datalink(&mut self, now: f64) -> Result<(), SessionError> {
        self.socket = Self::open_socket(self.remote)?;
        self.duml_seq = 0;
        self.udp_seq = self.base_seq;
        self.command_counter = 0;
        self.sequencer = new_sequencer(now);
        self.pump = AckPump::new(self.base_seq);
        self.depacketizer.reset();
        self.enable_not_before = None;
        self.health.note_datalink_rebuilt(now);
        Ok(())
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

    /// Queues an operator command for the next tick. Live-control SETs go through the
    /// mailbox: latest wins per opcode, one on the wire at a time, retransmitted once
    /// and settled the way the phones do it.
    pub fn send(&mut self, command: Command) {
        match command
            .opcode_key()
            .filter(|key| command::is_live_control(*key))
        {
            Some(key) => {
                let now = self.now();
                self.sequencer
                    .fire_set(key, command, !command.is_slider(), now);
            }
            None => self.sequencer.enqueue(command),
        }
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
        for outcome in self.sequencer.take_set_outcomes() {
            events.push(SessionEvent::Set(outcome));
        }
        for due in self.sequencer.tick(now) {
            if due == Outgoing::EnableLiveView && self.enable_not_before.is_some_and(|at| now < at)
            {
                continue;
            }
            self.dispatch(due)?;
        }
        if self.enable_not_before.is_some_and(|at| now >= at) {
            self.enable_not_before = None;
            self.dispatch(Outgoing::EnableLiveView)?;
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
            Recovery::ReopenDatalink | Recovery::FullRejoin => {
                // The desktop shell already owns a live SoftAP connection. Until its
                // BLE reconnect runner exists, a full rejoin must at least reopen UDP
                // instead of leaving the last frame frozen forever.
                self.reopen_datalink(now)?;
            }
            // A wedged decoder is the shell's to carry out.
            Recovery::RebuildDecoder => {}
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
                // Windows can report WSA_IO_PENDING (997) or WSAEINVAL (10022) from a
                // timed synchronous UDP receive while the camera changes its stream.
                // Neither invalidates the socket; treat both as an empty poll so the
                // watchdog can reopen the datalink if needed.
                Err(error) if matches!(error.raw_os_error(), Some(997 | 10022)) => {
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
                // Match the phone spine: establish app presence and gimbal state before
                // subscribing. Some bodies acknowledge telemetry without opening video
                // until this registration burst has arrived.
                self.send_registration()?;
                // Ask for the pushes the HUD needs before anything else is queued: the
                // camera only sends its available-value lists to a subscriber.
                self.send_subscriptions()?;
                self.enable_not_before = Some(now + SUBSCRIBE_SETTLE);
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
                    if let Some(key) = command::opcode_key(frame.cmd_set, frame.cmd_id) {
                        self.sequencer.reply(key, frame.seq, now);
                    }
                    events.push(SessionEvent::Frame(frame));
                }
                for outcome in self.sequencer.take_set_outcomes() {
                    events.push(SessionEvent::Set(outcome));
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
    fn send_registration(&mut self) -> Result<(), SessionError> {
        for command in [Command::AppPresence, Command::GimbalInit] {
            let datagram = self.command_datagram(command)?;
            self.socket.send(&datagram)?;
            self.send_ack()?;
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
        self.send_ack()?;
        Ok(())
    }

    /// Registration is a short burst; each write gets an immediate window ACK, matching
    /// the phone driver, before the regular 40 Hz pump takes over.
    fn send_ack(&mut self) -> Result<(), SessionError> {
        let datagram = self.pump.datagram(self.session_id)?;
        self.socket.send(&datagram)?;
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
                let seq = self.duml_seq;
                let datagram = self.command_datagram(command)?;
                if let Some(key) = command
                    .opcode_key()
                    .filter(|key| command::is_live_control(*key))
                {
                    self.sequencer.note_transmitted(key, seq);
                }
                datagram
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

/// The sequencer with the core's mailbox behind it when the core is linked, and the
/// plain queue when it is not.
#[cfg(opc_core_linked)]
fn new_sequencer(now: f64) -> Sequencer {
    match crate::mailbox::CoreMailbox::new() {
        Some(mailbox) => Sequencer::with_policy(now, Box::new(mailbox)),
        None => Sequencer::new(now),
    }
}

#[cfg(not(opc_core_linked))]
fn new_sequencer(now: f64) -> Sequencer {
    Sequencer::new(now)
}
