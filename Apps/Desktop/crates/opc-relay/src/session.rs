//! The watcher join state machine.
//!
//! Mirrors the iOS watcher: connect, say hello, then live until the host stops sending.
//! Reconnect timing is not decided here — every deadline comes from the core's
//! [`WatcherPolicy`], so a desktop watcher cannot retry harder than a phone does, and it
//! never asks the camera for a keyframe.

use std::fmt;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use opc_core_sys as sys;

use crate::ffi::{
    self, ControlToken, FrameMeta, Hello, ProtocolInfo, RecoveryAction, RelayError, State,
    WatcherPolicy,
};
use crate::transport::{Transport, TransportError};

/// Where the watcher is in a join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Connecting,
    /// Waiting out rung `n` of the core's retry ladder.
    Reconnecting(i32),
    /// The host wants a passcode. The caller collects one and joins again.
    NeedsPasscode,
    Live,
    Failed(String),
}

/// The host to join, as discovery found it.
#[derive(Debug, Clone)]
pub struct JoinTarget {
    pub name: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
}

/// Watcher identity and credentials for one join.
#[derive(Debug, Clone)]
pub struct WatcherOptions {
    pub device_name: String,
    pub watcher_id: String,
    pub passcode: String,
    pub connect_timeout: Duration,
}

impl Default for WatcherOptions {
    fn default() -> Self {
        Self {
            device_name: "Desktop".to_string(),
            watcher_id: String::new(),
            passcode: String::new(),
            connect_timeout: Duration::from_secs(5),
        }
    }
}

/// What the shell is told as a join progresses. Every method has a default so a caller
/// can implement only the parts it draws.
pub trait WatcherObserver {
    fn status_changed(&mut self, _status: &Status) {}
    fn accepted(&mut self, _hello: &Hello) {}
    fn state_changed(&mut self, _state: &State) {}
    fn token_changed(&mut self, _token: &ControlToken) {}
    /// One HEVC access unit. `parameter_sets` is non-empty only on a keyframe.
    fn picture(&mut self, _meta: &FrameMeta, _parameter_sets: &[Vec<u8>], _access_unit: &[u8]) {}
    /// Returns false to leave the feed.
    fn should_continue(&mut self) -> bool {
        true
    }
}

#[derive(Debug)]
pub enum SessionError {
    Relay(RelayError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relay(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<RelayError> for SessionError {
    fn from(error: RelayError) -> Self {
        Self::Relay(error)
    }
}

/// One join to one host.
#[derive(Debug)]
pub struct WatcherSession {
    info: ProtocolInfo,
    target: JoinTarget,
    options: WatcherOptions,
    policy: WatcherPolicy,
    transport: Option<Transport>,
    status: Status,
    state: State,
    token: ControlToken,
    started: Instant,
}

impl WatcherSession {
    pub fn new(info: ProtocolInfo, target: JoinTarget, options: WatcherOptions) -> Self {
        let started = Instant::now();
        Self {
            info,
            target,
            options,
            policy: WatcherPolicy::new(0.0),
            transport: None,
            status: Status::Connecting,
            state: State::default(),
            token: ControlToken::default(),
            started,
        }
    }

    /// Monotonic seconds since the join began, matching the core's clock contract.
    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn token(&self) -> &ControlToken {
        &self.token
    }

    /// True once the host has handed this watcher the control token.
    pub fn can_control(&self) -> bool {
        self.status == Status::Live && self.token.holder_is_recipient
    }

    /// Runs until the host is gone for good, the caller leaves, or a passcode is needed.
    pub fn run(&mut self, observer: &mut dyn WatcherObserver) -> Result<(), SessionError> {
        self.connect(observer);
        loop {
            if matches!(self.status, Status::Failed(_) | Status::NeedsPasscode) {
                return Ok(());
            }
            if !observer.should_continue() {
                self.policy.stop();
                self.transport = None;
                return Ok(());
            }

            // Take the result before touching `self` again so the transport borrow ends.
            let incoming = match self.transport.as_mut() {
                Some(transport) => transport.next_message(),
                None => Ok(None),
            };
            match incoming {
                Ok(Some(message)) => {
                    self.handle(message.kind, &message.payload, observer)?;
                    continue;
                }
                Ok(None) => {}
                Err(error) => self.connection_lost(&error.to_string(), observer),
            }

            let now = self.now();
            match self.policy.tick(now) {
                RecoveryAction::Reconnect => self.connect(observer),
                RecoveryAction::Exhausted => self.fail(
                    "Could not reconnect. Check that both devices are on the same camera \
                     Wi-Fi and sharing is on, then choose the feed again.",
                    observer,
                ),
                RecoveryAction::None => {
                    if self.policy.retry_pending() {
                        self.begin_reconnect(observer);
                    }
                }
            }
        }
    }

    fn set_status(&mut self, status: Status, observer: &mut dyn WatcherObserver) {
        if self.status == status {
            return;
        }
        self.status = status;
        observer.status_changed(&self.status);
    }

    fn connect(&mut self, observer: &mut dyn WatcherObserver) {
        self.transport = None;
        let retry = self.policy.retry_count();
        let status = if retry > 0 {
            Status::Reconnecting(retry)
        } else {
            Status::Connecting
        };
        self.set_status(status, observer);

        match Transport::connect(
            &self.target.addresses,
            self.target.port,
            self.options.connect_timeout,
        ) {
            Ok(transport) => {
                self.transport = Some(transport);
                if let Err(error) = self.send_hello() {
                    self.connection_lost(&error.to_string(), observer);
                }
            }
            Err(error) => self.connection_lost(&error.to_string(), observer),
        }
    }

    fn send_hello(&mut self) -> Result<(), TransportError> {
        let payload = ffi::encode_hello(
            &self.options.device_name,
            &self.options.passcode,
            &self.options.watcher_id,
        )?;
        match self.transport.as_mut() {
            Some(transport) => transport.send(sys::OPC_RELAY_KIND_HELLO, &payload),
            None => Err(TransportError::Closed),
        }
    }

    fn handle(
        &mut self,
        kind: u8,
        payload: &[u8],
        observer: &mut dyn WatcherObserver,
    ) -> Result<(), SessionError> {
        match kind {
            sys::OPC_RELAY_KIND_HELLO => {
                let hello = ffi::decode_hello(payload)?;
                if hello.version != self.info.version {
                    self.fail(
                        "Update OpenPocketCine on both devices to watch this feed.",
                        observer,
                    );
                    return Ok(());
                }
                let now = self.now();
                self.policy.connected(now);
                self.set_status(Status::Live, observer);
                observer.accepted(&hello);
            }
            sys::OPC_RELAY_KIND_JOIN_DENIED => {
                let denied = ffi::decode_join_denied(payload)?;
                if denied.passcode_required {
                    self.policy.stop();
                    self.transport = None;
                    self.set_status(Status::NeedsPasscode, observer);
                } else {
                    self.fail(&denied.reason, observer);
                }
            }
            sys::OPC_RELAY_KIND_STATE => {
                self.state = ffi::decode_state(payload)?;
                let now = self.now();
                self.policy.received(now, false);
                observer.state_changed(&self.state);
            }
            sys::OPC_RELAY_KIND_CONTROL_TOKEN => {
                self.token = ffi::decode_control_token(payload)?;
                let now = self.now();
                self.policy.received(now, false);
                observer.token_changed(&self.token);
            }
            sys::OPC_RELAY_KIND_FRAME => self.handle_frame(payload, observer)?,
            _ => {}
        }
        Ok(())
    }

    fn handle_frame(
        &mut self,
        payload: &[u8],
        observer: &mut dyn WatcherObserver,
    ) -> Result<(), SessionError> {
        let blob = ffi::decode_frame_blob(payload)?;
        if self.status != Status::Live {
            return Ok(());
        }
        let now = self.now();
        if self.policy.is_falling_behind(blob.meta.encoded_at, now) {
            self.connection_lost("The shared feed fell behind live delivery.", observer);
            return Ok(());
        }
        let sets = if blob.meta.parameter_set_count > 0 {
            ffi::parameter_sets(
                &payload[blob.meta_json.clone()],
                blob.meta.parameter_set_count,
            )?
        } else {
            Vec::new()
        };
        observer.picture(&blob.meta, &sets, &payload[blob.access_unit.clone()]);
        // Arrival stands in for presentation until the desktop decoder lands; when it
        // does, this flag must move to the presented callback the way iOS does it.
        self.policy.received(now, true);
        Ok(())
    }

    fn begin_reconnect(&mut self, observer: &mut dyn WatcherObserver) {
        self.transport = None;
        self.token = ControlToken::default();
        let retry = self.policy.retry_count().max(1);
        self.set_status(Status::Reconnecting(retry), observer);
    }

    fn connection_lost(&mut self, reason: &str, observer: &mut dyn WatcherObserver) {
        self.transport = None;
        let now = self.now();
        if self.policy.disconnected(now) == RecoveryAction::Exhausted {
            self.fail(
                &format!("Could not reconnect after {reason} Check camera Wi-Fi and sharing."),
                observer,
            );
        } else {
            self.begin_reconnect(observer);
        }
    }

    fn fail(&mut self, message: &str, observer: &mut dyn WatcherObserver) {
        self.policy.stop();
        self.transport = None;
        self.token = ControlToken::default();
        self.set_status(Status::Failed(message.to_string()), observer);
    }

    /// Asks the host for the control token. Ignored unless the host allows requests.
    pub fn request_control(&mut self) -> Result<(), TransportError> {
        if self.status != Status::Live || !self.state.allows_control_requests {
            return Ok(());
        }
        self.send(sys::OPC_RELAY_KIND_REQUEST_CONTROL, &[])
    }

    pub fn release_control(&mut self) -> Result<(), TransportError> {
        if !self.can_control() {
            return Ok(());
        }
        self.send(sys::OPC_RELAY_KIND_RELEASE_CONTROL, &[])
    }

    /// Sends a camera write. Silently ignored unless this watcher holds the token.
    pub fn send_command(&mut self, command: ffi::Command) -> Result<(), TransportError> {
        if !self.can_control() {
            return Ok(());
        }
        let payload = ffi::encode_command(command)?;
        self.send(sys::OPC_RELAY_KIND_COMMAND, &payload)
    }

    fn send(&mut self, kind: u8, payload: &[u8]) -> Result<(), TransportError> {
        match self.transport.as_mut() {
            Some(transport) => transport.send(kind, payload),
            None => Err(TransportError::Closed),
        }
    }
}
