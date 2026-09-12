//! What the datalink sends, and when.
//!
//! Deliberately free of sockets and of the core: this decides *which* datagram is due,
//! and the driver turns that into bytes. Keeping the two apart means the rules that are
//! easiest to get wrong — how often the pump fires, and how many times live view is
//! enabled — are testable against a fake clock with nothing else in the way.

use std::collections::VecDeque;

use crate::Command;

/// Forty hertz. Slower and the camera's send window closes; the contract is in
/// `docs/live-session.md`.
pub const ACK_INTERVAL: f64 = 1.0 / 40.0;
/// How long to wait for a handshake answer before asking again.
pub const HANDSHAKE_RETRY: f64 = 0.5;
/// Giving up on the handshake entirely.
pub const HANDSHAKE_DEADLINE: f64 = 10.0;

/// One datagram the driver should put on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    /// Session open. Repeats until the camera answers.
    Handshake,
    /// The window acknowledgement, 40 times a second.
    Ack,
    /// `0x09/0xa8`. Sent **once** per session by this sequencer; every later enable
    /// belongs to the watchdog.
    EnableLiveView,
    /// An operator command.
    Command(Command),
}

/// Where the datalink is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Asking to open a session.
    Handshaking,
    /// Session open, pump running, waiting for the first picture.
    Waiting,
    /// A picture has arrived.
    Live,
    /// The camera never answered.
    Unreachable,
}

/// Decides what the datalink sends next.
#[derive(Debug)]
pub struct Sequencer {
    phase: Phase,
    started: f64,
    last_handshake: Option<f64>,
    last_ack: Option<f64>,
    enabled: bool,
    queue: VecDeque<Command>,
}

impl Sequencer {
    pub fn new(now: f64) -> Self {
        Self {
            phase: Phase::Handshaking,
            started: now,
            last_handshake: None,
            last_ack: None,
            enabled: false,
            queue: VecDeque::new(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// True once live view has been asked for. It is asked for exactly once.
    pub fn enabled_live_view(&self) -> bool {
        self.enabled
    }

    /// Starts over after the datalink was torn down.
    ///
    /// Live view is enabled once per session, so a new session gets a new enable — that
    /// is not the same as the connect path sending two.
    pub fn restart(&mut self, now: f64) {
        self.phase = Phase::Handshaking;
        self.started = now;
        self.last_handshake = None;
        self.last_ack = None;
        self.enabled = false;
    }

    /// The camera answered the session open.
    pub fn note_handshake_reply(&mut self, now: f64) {
        if self.phase == Phase::Handshaking {
            self.phase = Phase::Waiting;
            // The pump starts on the next tick rather than instantly, so a handshake and
            // an acknowledgement never leave in the same breath.
            self.last_ack = Some(now);
        }
    }

    /// A video packet arrived.
    pub fn note_picture(&mut self) {
        if self.phase == Phase::Waiting {
            self.phase = Phase::Live;
        }
    }

    /// Queues an operator command. It goes out on the next tick, on the same queue as
    /// the pump, because every write has to serialise.
    pub fn enqueue(&mut self, command: Command) {
        self.queue.push_back(command);
    }

    pub fn pending_commands(&self) -> usize {
        self.queue.len()
    }

    /// What is due now.
    pub fn tick(&mut self, now: f64) -> Vec<Outgoing> {
        let mut out = Vec::new();
        match self.phase {
            Phase::Unreachable => return out,
            Phase::Handshaking => {
                if now - self.started >= HANDSHAKE_DEADLINE {
                    self.phase = Phase::Unreachable;
                    return out;
                }
                let due = self
                    .last_handshake
                    .is_none_or(|sent| now - sent >= HANDSHAKE_RETRY);
                if due {
                    self.last_handshake = Some(now);
                    out.push(Outgoing::Handshake);
                }
                // Nothing else may go out before the session is open.
                return out;
            }
            Phase::Waiting | Phase::Live => {}
        }

        if self.last_ack.is_none_or(|sent| now - sent >= ACK_INTERVAL) {
            self.last_ack = Some(now);
            out.push(Outgoing::Ack);
        }

        // Enable-once, after the first acknowledgement so the camera has a live window
        // to answer into.
        if !self.enabled {
            self.enabled = true;
            out.push(Outgoing::EnableLiveView);
        }

        while let Some(command) = self.queue.pop_front() {
            out.push(Outgoing::Command(command));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_opens_before_anything_else_goes_out() {
        let mut sequencer = Sequencer::new(0.0);
        assert_eq!(sequencer.tick(0.0), vec![Outgoing::Handshake]);
        // No acknowledgement and no enable until the camera answers.
        assert!(sequencer.tick(0.1).is_empty());
        assert!(sequencer.tick(0.4).is_empty());
    }

    #[test]
    fn the_handshake_repeats_until_it_is_answered() {
        let mut sequencer = Sequencer::new(0.0);
        assert_eq!(sequencer.tick(0.0), vec![Outgoing::Handshake]);
        assert_eq!(sequencer.tick(0.5), vec![Outgoing::Handshake]);
        assert_eq!(sequencer.tick(1.0), vec![Outgoing::Handshake]);
        sequencer.note_handshake_reply(1.1);
        assert_eq!(sequencer.phase(), Phase::Waiting);
        assert!(!sequencer.tick(1.6).contains(&Outgoing::Handshake));
    }

    #[test]
    fn a_camera_that_never_answers_is_given_up_on() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.tick(0.0);
        assert_eq!(sequencer.phase(), Phase::Handshaking);
        sequencer.tick(HANDSHAKE_DEADLINE);
        assert_eq!(sequencer.phase(), Phase::Unreachable);
        assert!(sequencer.tick(HANDSHAKE_DEADLINE + 1.0).is_empty());
    }

    #[test]
    fn live_view_is_enabled_exactly_once() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.note_handshake_reply(0.0);

        let first = sequencer.tick(0.1);
        assert!(first.contains(&Outgoing::EnableLiveView));

        // Not on any later tick, not when the picture arrives, not ever again. Repeats
        // belong to the watchdog.
        for step in 1..200 {
            let now = 0.1 + f64::from(step) * ACK_INTERVAL;
            assert!(
                !sequencer.tick(now).contains(&Outgoing::EnableLiveView),
                "live view was enabled twice at {now}"
            );
        }
        sequencer.note_picture();
        assert!(!sequencer.tick(10.0).contains(&Outgoing::EnableLiveView));
        assert!(sequencer.enabled_live_view());
    }

    #[test]
    fn the_pump_runs_at_forty_hertz() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.note_handshake_reply(0.0);

        // Step at 1 ms and count acknowledgements over a second.
        let mut acks = 0;
        for step in 1..=1000 {
            let now = f64::from(step) / 1000.0;
            acks += sequencer
                .tick(now)
                .iter()
                .filter(|out| **out == Outgoing::Ack)
                .count();
        }
        assert!(
            (39..=41).contains(&acks),
            "{acks} acknowledgements in a second"
        );
    }

    #[test]
    fn a_tick_that_is_too_soon_sends_nothing() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.note_handshake_reply(0.0);
        sequencer.tick(0.1);
        assert!(sequencer.tick(0.101).is_empty(), "1 ms later is not due");
    }

    #[test]
    fn a_long_stall_does_not_burst_a_backlog_of_acknowledgements() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.note_handshake_reply(0.0);
        sequencer.tick(0.1);
        // The thread was away for a second. One acknowledgement is due, not forty.
        let caught_up = sequencer.tick(1.1);
        assert_eq!(
            caught_up
                .iter()
                .filter(|out| **out == Outgoing::Ack)
                .count(),
            1
        );
    }

    #[test]
    fn commands_ride_the_same_queue_as_the_pump() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.note_handshake_reply(0.0);
        sequencer.tick(0.1);

        sequencer.enqueue(Command::RecordStart);
        sequencer.enqueue(Command::ZoomFactor(2.0));
        assert_eq!(sequencer.pending_commands(), 2);

        let out = sequencer.tick(0.2);
        assert_eq!(
            out.iter()
                .filter(|item| matches!(item, Outgoing::Command(_)))
                .count(),
            2
        );
        assert_eq!(sequencer.pending_commands(), 0);
        // In the order the operator pressed them.
        let commands: Vec<_> = out
            .iter()
            .filter_map(|item| match item {
                Outgoing::Command(command) => Some(*command),
                _ => None,
            })
            .collect();
        assert_eq!(
            commands,
            vec![Command::RecordStart, Command::ZoomFactor(2.0)]
        );
    }

    #[test]
    fn a_command_queued_before_the_session_opens_waits_for_it() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.enqueue(Command::RecordStart);
        assert!(!sequencer
            .tick(0.0)
            .iter()
            .any(|item| matches!(item, Outgoing::Command(_))));
        sequencer.note_handshake_reply(0.1);
        assert!(sequencer
            .tick(0.2)
            .iter()
            .any(|item| matches!(item, Outgoing::Command(_))));
    }

    #[test]
    fn a_restart_opens_a_new_session_and_enables_once_more() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.note_handshake_reply(0.0);
        assert!(sequencer.tick(0.1).contains(&Outgoing::EnableLiveView));
        sequencer.note_picture();

        // A torn-down datalink is a new session: handshake again, then one enable.
        sequencer.restart(10.0);
        assert_eq!(sequencer.phase(), Phase::Handshaking);
        assert!(!sequencer.enabled_live_view());
        assert_eq!(sequencer.tick(10.0), vec![Outgoing::Handshake]);
        sequencer.note_handshake_reply(10.1);
        assert!(sequencer.tick(10.2).contains(&Outgoing::EnableLiveView));
        // And still only once for this new session.
        for step in 1..50 {
            let now = 10.2 + f64::from(step) * ACK_INTERVAL;
            assert!(!sequencer.tick(now).contains(&Outgoing::EnableLiveView));
        }
    }

    #[test]
    fn the_first_picture_moves_the_phase_to_live() {
        let mut sequencer = Sequencer::new(0.0);
        sequencer.note_handshake_reply(0.0);
        assert_eq!(sequencer.phase(), Phase::Waiting);
        sequencer.note_picture();
        assert_eq!(sequencer.phase(), Phase::Live);
    }
}
