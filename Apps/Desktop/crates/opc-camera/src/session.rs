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
use crate::packed::DumlFrame;
use crate::sequence::{Outgoing, Phase, Sequencer};
use crate::transport::{self, AckPump, PktType};
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
    /// The camera never answered the handshake.
    Unreachable,
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
        Ok(events)
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

        if transport::is_handshake(datagram) {
            let opening = self.sequencer.phase() == Phase::Handshaking;
            self.sequencer.note_handshake_reply(self.now());
            if opening {
                events.push(SessionEvent::Opened);
            }
            return Ok(());
        }

        match PktType::of(datagram) {
            Some(PktType::Video) => {
                self.sequencer.note_picture();
                // The whole datagram, header included: the core reads the packet type at
                // byte 6 and the fragment index at bytes 16 to 18, and the encoded body
                // only starts at byte 20.
                if let Some(unit) = self.depacketizer.feed(datagram) {
                    events.push(SessionEvent::Picture(unit));
                }
            }
            Some(PktType::Telemetry | PktType::AckedData | PktType::Command) => {
                for frame in transport::scan_frames(datagram)? {
                    events.push(SessionEvent::Frame(frame));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, due: Outgoing) -> Result<(), SessionError> {
        let datagram = match due {
            Outgoing::Handshake => {
                transport::handshake(self.session_id, self.udp_seq, self.base_seq)?
            }
            Outgoing::Ack => self.pump.datagram(self.session_id)?,
            Outgoing::EnableLiveView => self.command_datagram(Command::LiveViewEnable)?,
            Outgoing::Command(command) => self.command_datagram(command)?,
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
