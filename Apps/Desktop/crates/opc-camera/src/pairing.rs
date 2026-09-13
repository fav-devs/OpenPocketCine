//! The Bluetooth handshake that ends with the camera's Wi-Fi credentials.
//!
//! Sockets, GATT and the clock stay outside. This decides only what the next step is,
//! given what the camera has said so far, which makes the whole flow — including the
//! branch where the operator has to press a button on the camera — testable with no
//! hardware in the room.
//!
//! The sequence, from the protocol notes:
//!
//! 1. Wake the session.
//! 2. Offer a pairing PIN. The camera answers `00 01` if it already knows this client,
//!    or `00 02` if a human has to approve on the body.
//! 3. If it asks for approval, it sends its own `0x07/0x46` request; answer that.
//! 4. Ask it to wake its access point.
//! 5. Read the Wi-Fi name, then the password.

use std::time::Duration;

/// Command set and identifier of a frame, which is all this machine needs to route one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub cmd_set: u8,
    pub cmd_id: u8,
    pub seq: u16,
    pub is_request: bool,
    pub payload: Vec<u8>,
}

/// What to write to the camera next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairStep {
    /// Open the session.
    Wake,
    /// Offer the PIN.
    SetPin,
    /// Answer the camera's approval request, echoing its sequence.
    ApproveWith(u16),
    /// Ask the camera to bring its access point up.
    WakeAccessPoint,
    AskSsid,
    AskPassword,
}

/// Where the pairing has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairState {
    Waking,
    Pinning,
    /// The camera wants a human to approve this client on the body.
    AwaitingApproval,
    WakingAccessPoint,
    AskingSsid,
    AskingPassword,
    Done {
        ssid: String,
        password: String,
    },
    Failed(String),
}

/// How long any one step may go unanswered before it is retried.
pub const STEP_RETRY: Duration = Duration::from_secs(2);
/// How long the whole exchange may take. Approval waits for a human, so it is generous.
pub const PAIR_DEADLINE: Duration = Duration::from_secs(120);

/// Drives the exchange.
#[derive(Debug)]
pub struct Pairing {
    state: PairState,
    started: f64,
    last_sent: Option<f64>,
    pending: Option<PairStep>,
    /// Held between the two credential replies; the name arrives before the password.
    ssid: String,
}

impl Pairing {
    pub fn new(now: f64) -> Self {
        Self {
            state: PairState::Waking,
            started: now,
            last_sent: None,
            pending: Some(PairStep::Wake),
            ssid: String::new(),
        }
    }

    pub fn state(&self) -> &PairState {
        &self.state
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.state, PairState::Done { .. } | PairState::Failed(_))
    }

    /// The credentials, once there are any.
    pub fn credentials(&self) -> Option<(&str, &str)> {
        match &self.state {
            PairState::Done { ssid, password } => Some((ssid, password)),
            _ => None,
        }
    }

    /// What to write now, if anything. Resends a step the camera has not answered.
    ///
    /// `0x00/0x2b` opens the BLE session but is not a request/reply gate: Pocket
    /// cameras commonly do not answer it.  Mimo and both phone shells therefore
    /// send `0x07/0x45` after the wake write, rather than waiting for a `0x00/0x2b`
    /// response that may never arrive.
    pub fn tick(&mut self, now: f64) -> Option<PairStep> {
        if self.is_finished() {
            return None;
        }
        if now - self.started >= PAIR_DEADLINE.as_secs_f64() {
            self.state = PairState::Failed(
                "The camera did not finish pairing. Bring it closer and try again.".to_string(),
            );
            return None;
        }
        // Waiting on a human is not a step to retry — the camera has already been asked.
        if self.state == PairState::AwaitingApproval && self.pending.is_none() {
            return None;
        }
        let due = match self.last_sent {
            None => true,
            Some(sent) => now - sent >= STEP_RETRY.as_secs_f64(),
        };
        if !due {
            return None;
        }
        let step = self.pending.clone().or_else(|| self.step_for_state())?;
        if step == PairStep::Wake {
            // Advance on the write, not on a reply. Keep SetPin pending so the
            // next driver tick sends it without waiting for STEP_RETRY.
            self.state = PairState::Pinning;
            self.pending = Some(PairStep::SetPin);
            self.last_sent = None;
        } else {
            self.pending = None;
            self.last_sent = Some(now);
        }
        Some(step)
    }

    fn step_for_state(&self) -> Option<PairStep> {
        match self.state {
            PairState::Waking => Some(PairStep::Wake),
            PairState::Pinning => Some(PairStep::SetPin),
            PairState::WakingAccessPoint => Some(PairStep::WakeAccessPoint),
            PairState::AskingSsid => Some(PairStep::AskSsid),
            PairState::AskingPassword => Some(PairStep::AskPassword),
            _ => None,
        }
    }

    /// Folds in one frame from the camera. `text` is the status string the core read out
    /// of the payload, which the driver supplies for the two credential replies.
    pub fn receive(&mut self, now: f64, reply: &Reply, text: Option<&str>) {
        let _ = now;
        match (self.state.clone(), reply.cmd_set, reply.cmd_id) {
            // The PIN answer decides whether a human has to get involved.
            (PairState::Pinning, 0x07, 0x45) => match reply.payload.get(1) {
                Some(0x01) => {
                    self.state = PairState::WakingAccessPoint;
                    self.advance(PairStep::WakeAccessPoint);
                }
                Some(0x02) => {
                    self.state = PairState::AwaitingApproval;
                    self.pending = None;
                }
                _ => {
                    self.state =
                        PairState::Failed("The camera refused the pairing request.".to_string());
                }
            },
            // The camera asks for approval with a request of its own; answer it and move on.
            (_, 0x07, 0x46) if reply.is_request => {
                self.state = PairState::WakingAccessPoint;
                self.advance(PairStep::ApproveWith(reply.seq));
            }
            (PairState::WakingAccessPoint, 0x53, 0x10) => {
                self.state = PairState::AskingSsid;
                self.advance(PairStep::AskSsid);
            }
            (PairState::AskingSsid, 0x07, 0x07) => match text {
                Some(name) if !name.is_empty() => {
                    self.ssid = name.to_string();
                    self.state = PairState::AskingPassword;
                    self.advance(PairStep::AskPassword);
                }
                _ => {
                    self.state =
                        PairState::Failed("The camera did not give a Wi-Fi name.".to_string());
                }
            },
            (PairState::AskingPassword, 0x07, 0x0E) => match text {
                Some(password) if !password.is_empty() => {
                    self.state = PairState::Done {
                        ssid: std::mem::take(&mut self.ssid),
                        password: password.to_string(),
                    };
                    self.pending = None;
                }
                _ => {
                    self.state =
                        PairState::Failed("The camera did not give a Wi-Fi password.".to_string());
                }
            },
            _ => {}
        }
    }

    /// Queues the next step and lets it go out immediately rather than after a retry.
    fn advance(&mut self, step: PairStep) {
        self.pending = Some(step);
        self.last_sent = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(cmd_set: u8, cmd_id: u8, payload: &[u8]) -> Reply {
        Reply {
            cmd_set,
            cmd_id,
            seq: 0x1234,
            is_request: false,
            payload: payload.to_vec(),
        }
    }

    fn request(cmd_set: u8, cmd_id: u8, seq: u16) -> Reply {
        Reply {
            cmd_set,
            cmd_id,
            seq,
            is_request: true,
            payload: Vec::new(),
        }
    }

    /// Walks a camera that already knows this client all the way to credentials.
    fn known_camera() -> Pairing {
        let mut pairing = Pairing::new(0.0);
        assert_eq!(pairing.tick(0.0), Some(PairStep::Wake));
        assert_eq!(pairing.tick(0.1), Some(PairStep::SetPin));
        pairing.receive(0.2, &reply(0x07, 0x45, &[0x00, 0x01]), None);
        assert_eq!(pairing.tick(0.2), Some(PairStep::WakeAccessPoint));
        pairing.receive(0.3, &reply(0x53, 0x10, &[0x01, 0, 0, 0]), None);
        assert_eq!(pairing.tick(0.3), Some(PairStep::AskSsid));
        pairing
    }

    #[test]
    fn a_known_camera_hands_over_credentials_without_a_human() {
        let mut pairing = known_camera();
        pairing.receive(0.4, &reply(0x07, 0x07, &[]), Some("OsmoPocket4-ABCDEF"));
        assert_eq!(pairing.tick(0.4), Some(PairStep::AskPassword));
        pairing.receive(0.5, &reply(0x07, 0x0E, &[]), Some("hunter2hunter2"));

        assert!(pairing.is_finished());
        assert_eq!(
            pairing.credentials(),
            Some(("OsmoPocket4-ABCDEF", "hunter2hunter2"))
        );
        assert_eq!(
            pairing.tick(1.0),
            None,
            "a finished pairing sends nothing more"
        );
    }

    #[test]
    fn a_new_camera_waits_for_the_operator_to_approve() {
        let mut pairing = Pairing::new(0.0);
        pairing.tick(0.0);
        pairing.tick(0.1);
        // `00 02`: a human has to press approve on the body.
        pairing.receive(0.2, &reply(0x07, 0x45, &[0x00, 0x02]), None);
        assert_eq!(pairing.state(), &PairState::AwaitingApproval);

        // Nothing is retried while waiting on a person — the camera has already been asked.
        assert_eq!(pairing.tick(10.0), None);
        assert_eq!(pairing.tick(30.0), None);

        // They press it; the camera asks us to confirm.
        pairing.receive(31.0, &request(0x07, 0x46, 0x0042), None);
        assert_eq!(pairing.tick(31.0), Some(PairStep::ApproveWith(0x0042)));
        assert_eq!(pairing.state(), &PairState::WakingAccessPoint);
    }

    #[test]
    fn an_unanswered_pin_is_retried_rather_than_abandoned() {
        let mut pairing = Pairing::new(0.0);
        assert_eq!(pairing.tick(0.0), Some(PairStep::Wake));
        assert_eq!(pairing.tick(0.1), Some(PairStep::SetPin));
        assert_eq!(pairing.tick(0.5), None, "too soon");
        assert_eq!(
            pairing.tick(2.1),
            Some(PairStep::SetPin),
            "retry after the pause"
        );
        assert_eq!(pairing.tick(4.2), Some(PairStep::SetPin));
    }

    #[test]
    fn each_step_follows_the_one_before_it_immediately() {
        let mut pairing = Pairing::new(0.0);
        pairing.tick(0.0);
        // Not held back by the retry pause: SessionWake is fire-and-forget.
        assert_eq!(pairing.tick(0.1), Some(PairStep::SetPin));
    }

    #[test]
    fn a_camera_that_never_finishes_is_given_up_on() {
        let mut pairing = Pairing::new(0.0);
        pairing.tick(0.0);
        assert!(!pairing.is_finished());
        pairing.tick(PAIR_DEADLINE.as_secs_f64() + 1.0);
        assert!(matches!(pairing.state(), PairState::Failed(_)));
        assert!(pairing.credentials().is_none());
    }

    #[test]
    fn a_refused_pin_fails_rather_than_hanging() {
        let mut pairing = Pairing::new(0.0);
        pairing.tick(0.0);
        pairing.tick(0.1);
        pairing.receive(0.2, &reply(0x07, 0x45, &[0x00, 0xFF]), None);
        assert!(matches!(pairing.state(), PairState::Failed(_)));
    }

    #[test]
    fn an_empty_credential_is_a_failure_not_an_empty_password() {
        let mut pairing = known_camera();
        pairing.receive(0.4, &reply(0x07, 0x07, &[]), Some(""));
        assert!(matches!(pairing.state(), PairState::Failed(_)));

        let mut pairing = known_camera();
        pairing.receive(0.4, &reply(0x07, 0x07, &[]), Some("OsmoPocket4-ABCDEF"));
        pairing.tick(0.4);
        pairing.receive(0.5, &reply(0x07, 0x0E, &[]), None);
        assert!(matches!(pairing.state(), PairState::Failed(_)));
    }

    #[test]
    fn a_frame_that_does_not_belong_to_this_step_is_ignored() {
        let mut pairing = Pairing::new(0.0);
        pairing.tick(0.0);
        // Telemetry, a stray reply for another step — none of it moves the flow.
        pairing.receive(0.1, &reply(0x02, 0xA5, &[0x00]), None);
        pairing.receive(0.1, &reply(0x07, 0x0E, &[]), Some("nope"));
        assert_eq!(pairing.state(), &PairState::Pinning);
    }
}
