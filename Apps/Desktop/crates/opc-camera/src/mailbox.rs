//! The SET mailbox: latest-wins per opcode, one generation on the wire.
//!
//! The decisions — what may launch, which ACK belongs to which generation, what a
//! silence means — are the core's `CameraSetMailbox`, the same one the phones run.
//! This drives its clock the way the phones do: retransmit once after 300 ms of
//! silence, settle at 2 s, and keep the latest pending write per opcode so a wheel
//! or a slider never queues. Without the core linked every SET goes out once, at
//! once, which is what the shell did before there was a mailbox.

use std::collections::HashMap;

use crate::Command;

/// Retransmit after this much ACK silence (Mimo retransmits before ACK too).
pub const RETRANSMIT_AFTER: f64 = 0.3;
/// Give up the waiter and accept a late ACK for the same generation.
pub const SETTLE_AFTER: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// Transmit now.
    Launch,
    /// The opcode is busy or rate-limited; keep only this latest pending.
    CoalescePending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckDecision {
    Accept,
    AcceptLate,
    DropSuperseded,
    DropUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutDecision {
    SubscribeMatches,
    WaitLate,
    LaunchPending,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingLaunch {
    Immediate,
    AfterHold,
    None,
}

/// What the core decides about a SET. The driver below only keeps the clock.
pub trait SetPolicy: std::fmt::Debug + Send {
    fn offer(&mut self, key: u16, urgent: bool, now: f64) -> Offer;
    fn begin_launch(&mut self, key: u16, now: f64);
    fn note_transmit(&mut self, key: u16, seq: u16);
    fn decide_ack(&mut self, key: u16, seq: u16) -> AckDecision;
    fn timeout(&mut self, key: u16, subscribe_matches: bool) -> TimeoutDecision;
    fn pending_launch(&mut self, key: u16, now: f64) -> PendingLaunch;
    fn hold_remaining(&self, key: u16, now: f64) -> f64;
    /// The zoom slider pipelines while a generation is open; everything else waits.
    fn pipelines(&self, key: u16) -> bool;
    fn reset(&mut self);
}

/// No mailbox: every SET goes straight out, once, and no reply is matched.
#[derive(Debug, Default)]
pub struct Direct;

impl SetPolicy for Direct {
    fn offer(&mut self, _key: u16, _urgent: bool, _now: f64) -> Offer {
        Offer::Launch
    }
    fn begin_launch(&mut self, _key: u16, _now: f64) {}
    fn note_transmit(&mut self, _key: u16, _seq: u16) {}
    fn decide_ack(&mut self, _key: u16, _seq: u16) -> AckDecision {
        AckDecision::DropUnknown
    }
    fn timeout(&mut self, _key: u16, _subscribe_matches: bool) -> TimeoutDecision {
        TimeoutDecision::Idle
    }
    fn pending_launch(&mut self, _key: u16, _now: f64) -> PendingLaunch {
        PendingLaunch::None
    }
    fn hold_remaining(&self, _key: u16, _now: f64) -> f64 {
        0.0
    }
    fn pipelines(&self, _key: u16) -> bool {
        false
    }
    fn reset(&mut self) {}
}

/// The core's `CameraSetMailbox`, through the facade.
#[cfg(opc_core_linked)]
pub struct CoreMailbox {
    handle: *mut std::ffi::c_void,
}

#[cfg(opc_core_linked)]
impl CoreMailbox {
    pub fn new() -> Option<Self> {
        // Safety: a fresh handle, or null.
        let handle = unsafe { opc_core_sys::opc_mailbox_new() };
        (!handle.is_null()).then_some(Self { handle })
    }
}

#[cfg(opc_core_linked)]
impl std::fmt::Debug for CoreMailbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CoreMailbox")
    }
}

// Safety: the handle is plain heap state touched from the datalink thread only.
#[cfg(opc_core_linked)]
unsafe impl Send for CoreMailbox {}

#[cfg(opc_core_linked)]
impl Drop for CoreMailbox {
    fn drop(&mut self) {
        // Safety: the handle came from `opc_mailbox_new` and is released once.
        unsafe { opc_core_sys::opc_mailbox_destroy(self.handle) }
    }
}

#[cfg(opc_core_linked)]
impl SetPolicy for CoreMailbox {
    fn offer(&mut self, key: u16, urgent: bool, now: f64) -> Offer {
        use opc_core_sys as sys;
        // Safety: plain values in; the handle is live for the life of `self`.
        let code =
            unsafe { sys::opc_mailbox_offer(self.handle, i32::from(key), i32::from(urgent), now) };
        if code == sys::OPC_MAILBOX_COALESCE {
            Offer::CoalescePending
        } else {
            Offer::Launch
        }
    }
    fn begin_launch(&mut self, key: u16, now: f64) {
        // Safety: as above.
        unsafe { opc_core_sys::opc_mailbox_begin_launch(self.handle, i32::from(key), now) }
    }
    fn note_transmit(&mut self, key: u16, seq: u16) {
        // Safety: as above.
        unsafe {
            opc_core_sys::opc_mailbox_note_transmit(self.handle, i32::from(key), i32::from(seq))
        }
    }
    fn decide_ack(&mut self, key: u16, seq: u16) -> AckDecision {
        use opc_core_sys as sys;
        // Safety: as above.
        let code =
            unsafe { sys::opc_mailbox_decide_ack(self.handle, i32::from(key), i32::from(seq)) };
        match code {
            sys::OPC_MAILBOX_ACK_ACCEPT => AckDecision::Accept,
            sys::OPC_MAILBOX_ACK_ACCEPT_LATE => AckDecision::AcceptLate,
            sys::OPC_MAILBOX_ACK_DROP_SUPERSEDED => AckDecision::DropSuperseded,
            _ => AckDecision::DropUnknown,
        }
    }
    fn timeout(&mut self, key: u16, subscribe_matches: bool) -> TimeoutDecision {
        use opc_core_sys as sys;
        // Safety: as above.
        let code = unsafe {
            sys::opc_mailbox_timeout(self.handle, i32::from(key), i32::from(subscribe_matches))
        };
        match code {
            sys::OPC_MAILBOX_TIMEOUT_SUBSCRIBE_MATCHES => TimeoutDecision::SubscribeMatches,
            sys::OPC_MAILBOX_TIMEOUT_WAIT_LATE => TimeoutDecision::WaitLate,
            sys::OPC_MAILBOX_TIMEOUT_LAUNCH_PENDING => TimeoutDecision::LaunchPending,
            _ => TimeoutDecision::Idle,
        }
    }
    fn pending_launch(&mut self, key: u16, now: f64) -> PendingLaunch {
        use opc_core_sys as sys;
        // Safety: as above.
        let code = unsafe { sys::opc_mailbox_pending_launch(self.handle, i32::from(key), now) };
        match code {
            sys::OPC_MAILBOX_PENDING_IMMEDIATE => PendingLaunch::Immediate,
            sys::OPC_MAILBOX_PENDING_AFTER_HOLD => PendingLaunch::AfterHold,
            _ => PendingLaunch::None,
        }
    }
    fn hold_remaining(&self, key: u16, now: f64) -> f64 {
        // Safety: as above.
        unsafe { opc_core_sys::opc_mailbox_hold_remaining(self.handle, i32::from(key), now) }
    }
    fn pipelines(&self, key: u16) -> bool {
        // Safety: a plain value in.
        unsafe { opc_core_sys::opc_mailbox_pipelines(i32::from(key)) != 0 }
    }
    fn reset(&mut self) {
        // Safety: as above.
        unsafe { opc_core_sys::opc_mailbox_reset(self.handle) }
    }
}

/// What became of a SET, for the shell to show.
#[derive(Debug, Clone, PartialEq)]
pub enum SetOutcome {
    /// The camera answered. `late` means after the settle window.
    Acked { command: Command, late: bool },
    /// No answer in the settle window; a late one is still accepted.
    Unanswered { command: Command },
    /// A newer write for the same opcode replaced it before it was answered.
    Superseded { command: Command },
}

#[derive(Debug)]
struct Inflight {
    command: Command,
    launched_at: f64,
    retransmits: bool,
    retransmitted: bool,
}

#[derive(Debug, Clone)]
struct Pending {
    command: Command,
    retransmits: bool,
}

/// Drives the mailbox's clock and keeps the commands it is about.
#[derive(Debug)]
pub struct SetDriver {
    policy: Box<dyn SetPolicy>,
    inflight: HashMap<u16, Inflight>,
    pending: HashMap<u16, Pending>,
    /// Settled without an answer; a late ACK still closes these.
    late: HashMap<u16, Command>,
    /// Launched by a reply, waiting for the next tick to go on the wire.
    due: Vec<Command>,
    outcomes: Vec<SetOutcome>,
}

impl SetDriver {
    pub fn new(policy: Box<dyn SetPolicy>) -> Self {
        Self {
            policy,
            inflight: HashMap::new(),
            pending: HashMap::new(),
            late: HashMap::new(),
            due: Vec::new(),
            outcomes: Vec::new(),
        }
    }

    /// Offers a SET. Returns the command to transmit now, if the mailbox lets it go.
    /// `urgent` is a chip tap or a key; a slider tick is not.
    pub fn fire(
        &mut self,
        key: u16,
        command: Command,
        urgent: bool,
        retransmits: bool,
        now: f64,
    ) -> Option<Command> {
        match self.policy.offer(key, urgent, now) {
            Offer::CoalescePending => {
                self.pending.insert(
                    key,
                    Pending {
                        command,
                        retransmits,
                    },
                );
                None
            }
            Offer::Launch => {
                if let Some(abandoned) = self.late.remove(&key) {
                    self.outcomes
                        .push(SetOutcome::Superseded { command: abandoned });
                }
                self.pending.remove(&key);
                Some(self.launch(key, command, retransmits, now))
            }
        }
    }

    fn launch(&mut self, key: u16, command: Command, retransmits: bool, now: f64) -> Command {
        if let Some(open) = self.inflight.remove(&key) {
            self.outcomes.push(SetOutcome::Superseded {
                command: open.command,
            });
        }
        self.policy.begin_launch(key, now);
        self.inflight.insert(
            key,
            Inflight {
                command,
                launched_at: now,
                retransmits,
                retransmitted: false,
            },
        );
        command
    }

    /// The datalink put a SET on the wire under this sequence number.
    pub fn note_transmitted(&mut self, key: u16, seq: u16) {
        self.policy.note_transmit(key, seq);
    }

    /// A reply arrived for `key` with `seq`. True when it closed a SET of ours.
    pub fn reply(&mut self, key: u16, seq: u16, now: f64) -> bool {
        match self.policy.decide_ack(key, seq) {
            AckDecision::Accept | AckDecision::AcceptLate => {
                if let Some(open) = self.inflight.remove(&key) {
                    self.outcomes.push(SetOutcome::Acked {
                        command: open.command,
                        late: false,
                    });
                } else if let Some(command) = self.late.remove(&key) {
                    self.outcomes.push(SetOutcome::Acked {
                        command,
                        late: true,
                    });
                }
                if let Some(next) = self.launch_pending(key, now) {
                    self.due.push(next);
                }
                true
            }
            AckDecision::DropSuperseded => true,
            AckDecision::DropUnknown => false,
        }
    }

    /// Retransmits, settles and pending launches that are due. The returned commands
    /// go on the wire now.
    pub fn tick(&mut self, now: f64) -> Vec<Command> {
        let mut out = std::mem::take(&mut self.due);
        let keys: Vec<u16> = self.inflight.keys().copied().collect();
        for key in keys {
            let Some(open) = self.inflight.get_mut(&key) else {
                continue;
            };
            if now - open.launched_at >= SETTLE_AFTER {
                let open = self.inflight.remove(&key).expect("just read");
                match self.policy.timeout(key, false) {
                    TimeoutDecision::WaitLate => {
                        self.outcomes.push(SetOutcome::Unanswered {
                            command: open.command,
                        });
                        self.late.insert(key, open.command);
                    }
                    TimeoutDecision::LaunchPending => {
                        self.outcomes.push(SetOutcome::Superseded {
                            command: open.command,
                        });
                    }
                    TimeoutDecision::SubscribeMatches | TimeoutDecision::Idle => {}
                }
                out.extend(self.launch_pending(key, now));
                continue;
            }
            if open.retransmits && !open.retransmitted && now - open.launched_at >= RETRANSMIT_AFTER
            {
                open.retransmitted = true;
                out.push(open.command);
            }
        }
        let waiting: Vec<u16> = self.pending.keys().copied().collect();
        for key in waiting {
            let blocked = self.inflight.contains_key(&key) && !self.policy.pipelines(key);
            if blocked {
                continue;
            }
            out.extend(self.launch_pending(key, now));
        }
        out
    }

    fn launch_pending(&mut self, key: u16, now: f64) -> Option<Command> {
        match self.policy.pending_launch(key, now) {
            PendingLaunch::Immediate => {
                let next = self.pending.remove(&key)?;
                Some(self.launch(key, next.command, next.retransmits, now))
            }
            PendingLaunch::AfterHold => None,
            PendingLaunch::None => {
                self.pending.remove(&key);
                None
            }
        }
    }

    /// What settled since the last call.
    pub fn take_outcomes(&mut self) -> Vec<SetOutcome> {
        std::mem::take(&mut self.outcomes)
    }

    pub fn in_flight(&self) -> usize {
        self.inflight.len()
    }

    /// Starts over after the datalink was torn down.
    pub fn reset(&mut self) {
        self.policy.reset();
        self.inflight.clear();
        self.pending.clear();
        self.late.clear();
        self.due.clear();
        self.outcomes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The shape of the core's mailbox, enough to drive the clock: one open generation
    /// per key, latest pending, a 100 ms hold on sliders, late ACKs for the open seqs.
    #[derive(Debug, Default)]
    struct Latest {
        open: HashMap<u16, (HashSet<u16>, bool)>,
        pending: HashMap<u16, bool>,
        last: HashMap<u16, f64>,
    }

    impl SetPolicy for Latest {
        fn offer(&mut self, key: u16, urgent: bool, now: f64) -> Offer {
            if self.open.get(&key).is_some_and(|(_, late)| !late) {
                *self.pending.entry(key).or_default() |= urgent;
                return Offer::CoalescePending;
            }
            if !urgent && self.last.get(&key).is_some_and(|last| now - last < 0.1) {
                *self.pending.entry(key).or_default() |= urgent;
                return Offer::CoalescePending;
            }
            self.pending.remove(&key);
            Offer::Launch
        }
        fn begin_launch(&mut self, key: u16, now: f64) {
            self.pending.remove(&key);
            self.last.insert(key, now);
            self.open.insert(key, (HashSet::new(), false));
        }
        fn note_transmit(&mut self, key: u16, seq: u16) {
            if let Some((seqs, _)) = self.open.get_mut(&key) {
                seqs.insert(seq);
            }
        }
        fn decide_ack(&mut self, key: u16, seq: u16) -> AckDecision {
            match self.open.get(&key) {
                Some((seqs, late)) if seqs.contains(&seq) => {
                    let late = *late;
                    self.open.remove(&key);
                    if late {
                        AckDecision::AcceptLate
                    } else {
                        AckDecision::Accept
                    }
                }
                _ => AckDecision::DropUnknown,
            }
        }
        fn timeout(&mut self, key: u16, _matches: bool) -> TimeoutDecision {
            if !self.open.contains_key(&key) {
                return TimeoutDecision::Idle;
            }
            if self.pending.contains_key(&key) {
                self.open.remove(&key);
                return TimeoutDecision::LaunchPending;
            }
            if let Some(open) = self.open.get_mut(&key) {
                open.1 = true;
            }
            TimeoutDecision::WaitLate
        }
        fn pending_launch(&mut self, key: u16, now: f64) -> PendingLaunch {
            let Some(urgent) = self.pending.get(&key).copied() else {
                return PendingLaunch::None;
            };
            if urgent || self.hold_remaining(key, now) <= 0.0 {
                PendingLaunch::Immediate
            } else {
                PendingLaunch::AfterHold
            }
        }
        fn hold_remaining(&self, key: u16, now: f64) -> f64 {
            self.last
                .get(&key)
                .map_or(0.0, |last| (0.1 - (now - last)).max(0.0))
        }
        fn pipelines(&self, _key: u16) -> bool {
            false
        }
        fn reset(&mut self) {
            *self = Self::default();
        }
    }

    const ISO: u16 = 0x0230;

    fn driver() -> SetDriver {
        SetDriver::new(Box::new(Latest::default()))
    }

    #[test]
    fn a_set_is_retransmitted_once_after_silence_and_an_ack_closes_it() {
        let mut driver = driver();
        let iso = Command::SetIsoIndex(5);
        assert_eq!(driver.fire(ISO, iso, true, true, 0.0), Some(iso));
        driver.note_transmitted(ISO, 7);
        assert!(driver.tick(0.2).is_empty(), "too soon to retransmit");
        assert_eq!(driver.tick(0.31), vec![iso], "one retransmit at 300 ms");
        driver.note_transmitted(ISO, 8);
        assert!(driver.tick(0.7).is_empty(), "never a second");
        assert!(
            driver.reply(ISO, 8, 0.8),
            "the retransmit's seq belongs to the generation"
        );
        assert_eq!(
            driver.take_outcomes(),
            vec![SetOutcome::Acked {
                command: iso,
                late: false
            }]
        );
        assert_eq!(driver.in_flight(), 0);
        assert!(!driver.reply(ISO, 7, 0.9), "a stale seq is nobody's");
    }

    #[test]
    fn a_wheel_keeps_only_its_latest_step_and_sends_it_after_the_ack() {
        let mut driver = driver();
        assert_eq!(
            driver.fire(ISO, Command::SetIsoIndex(3), true, true, 0.0),
            Some(Command::SetIsoIndex(3))
        );
        driver.note_transmitted(ISO, 1);
        assert_eq!(
            driver.fire(ISO, Command::SetIsoIndex(4), true, true, 0.01),
            None
        );
        assert_eq!(
            driver.fire(ISO, Command::SetIsoIndex(5), true, true, 0.02),
            None
        );
        assert!(
            driver.tick(0.05).is_empty(),
            "the open generation blocks the key"
        );
        assert!(driver.reply(ISO, 1, 0.1));
        // The ACK launches only the newest pending step; the middle one never went out.
        assert_eq!(driver.tick(0.1), vec![Command::SetIsoIndex(5)]);
        let outcomes = driver.take_outcomes();
        assert!(outcomes.contains(&SetOutcome::Acked {
            command: Command::SetIsoIndex(3),
            late: false
        }));
    }

    #[test]
    fn silence_settles_at_two_seconds_and_a_late_ack_still_counts() {
        let mut driver = driver();
        let ev = Command::SetEv(3);
        driver.fire(0x022A, ev, true, false, 0.0);
        driver.note_transmitted(0x022A, 9);
        assert!(
            driver.tick(1.9).is_empty(),
            "no retransmit for a write that does not"
        );
        assert!(driver.tick(2.0).is_empty());
        assert_eq!(
            driver.take_outcomes(),
            vec![SetOutcome::Unanswered { command: ev }]
        );
        assert!(driver.reply(0x022A, 9, 2.5));
        assert_eq!(
            driver.take_outcomes(),
            vec![SetOutcome::Acked {
                command: ev,
                late: true
            }]
        );
    }

    #[test]
    fn a_pending_write_replaces_a_set_that_timed_out() {
        let mut driver = driver();
        driver.fire(ISO, Command::SetIsoIndex(3), true, true, 0.0);
        driver.note_transmitted(ISO, 1);
        assert_eq!(
            driver.fire(ISO, Command::SetIsoIndex(6), true, true, 1.0),
            None
        );
        driver.tick(1.31);
        assert_eq!(driver.tick(2.0), vec![Command::SetIsoIndex(6)]);
        assert_eq!(
            driver.take_outcomes(),
            vec![SetOutcome::Superseded {
                command: Command::SetIsoIndex(3)
            }]
        );
    }

    #[test]
    fn without_the_core_every_set_goes_straight_out() {
        let mut driver = SetDriver::new(Box::new(Direct));
        assert_eq!(
            driver.fire(ISO, Command::SetIsoIndex(3), false, true, 0.0),
            Some(Command::SetIsoIndex(3))
        );
        assert_eq!(
            driver.fire(ISO, Command::SetIsoIndex(4), false, true, 0.0),
            Some(Command::SetIsoIndex(4))
        );
        assert!(!driver.reply(ISO, 1, 0.1));
    }
}
