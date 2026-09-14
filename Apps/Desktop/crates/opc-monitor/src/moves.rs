//! Programmed gimbal moves: A → B, optionally → C, each leg over a chosen duration,
//! driven with native timed targets (`0x04/0x14`).
//!
//! Transcribed from the core's `GimbalMoveEngine` and the timing contract in
//! `docs/programmed-moves.md`, without the parts the desktop does not do yet: no
//! Bézier smoothing at B, no pause and resume, and no fitted-delay verification of
//! B. What is kept is everything that keeps the camera safe and the take honest:
//! the approach to A in steps under 120°, the two-second hold, legs split into native
//! sub-moves when they span 180° or 25.5 s, every target checked against fresh actual
//! pan so the firmware can never pick the route through the missing sector, and a
//! final check that the camera stopped where it was sent.
//!
//! Status: experimental; timing and positional accuracy are not qualified on a body.

use opc_camera::{Command, Status};

/// The reachable pan arc and tilt range, in display `0x04/0x05` degrees.
pub const PAN_MIN: f64 = -48.0;
pub const PAN_MAX: f64 = 225.0;
pub const TILT_MIN: f64 = -44.0;
pub const TILT_MAX: f64 = 70.0;
/// Middle of the unreachable arc in raw yaw; raw yaw below it is the long side.
pub const GAP_MID: f64 = (PAN_MAX + PAN_MIN - 360.0) / 2.0;

pub const MIN_DURATION: f64 = 0.5;
pub const MAX_DURATION: f64 = 120.0;
/// The firmware's one-command limit.
pub const NATIVE_MAX: f64 = 25.5;
pub const ARRIVE_DEG: f64 = 0.15;
pub const HOLD_SECONDS: f64 = 2.0;
/// Feedback older than this is not a pose to dispatch against.
pub const FRESH_SECONDS: f64 = 0.3;
/// A boundary dispatch later than this invalidates the take. The phones allow 20 ms
/// from a dedicated scheduler; the window's draw loop gets twice that.
pub const LATE_SECONDS: f64 = 0.04;

/// One gimbal pose: unwrapped yaw, display tilt, and the native pitch a target takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Waypoint {
    pub yaw: f64,
    pub pitch: f64,
    pub native_pitch: f64,
}

impl Waypoint {
    /// Raw `0x04/0x05` yaw (±180) onto the reachable interval, so A→B is plain
    /// subtraction and a straight line cannot cross the gap.
    pub fn unwrap_yaw(raw: f64) -> f64 {
        if raw < GAP_MID {
            raw + 360.0
        } else {
            raw
        }
    }

    /// The body's last attitude, if it has reported one.
    pub fn from_status(status: &Status) -> Option<Self> {
        let yaw = status.gimbal_yaw_tenth?;
        let pitch = status.gimbal_pitch_tenth?;
        let native = status.gimbal_native_pitch_tenth?;
        Some(Self {
            yaw: Self::unwrap_yaw(f64::from(yaw) / 10.0),
            pitch: f64::from(pitch) / 10.0,
            native_pitch: f64::from(native) / 10.0,
        })
    }

    pub fn is_reachable(&self) -> bool {
        self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.native_pitch.is_finite()
            && (PAN_MIN..=PAN_MAX).contains(&self.yaw)
            && (TILT_MIN..=TILT_MAX).contains(&self.pitch)
            && (-180.0..=180.0).contains(&self.native_pitch)
    }

    pub fn label(&self) -> String {
        format!("pan {:+.1}° · tilt {:+.1}°", self.yaw, self.pitch)
    }

    fn distance(&self, other: &Self) -> f64 {
        (other.yaw - self.yaw).hypot(other.pitch - self.pitch)
    }

    fn lerp(&self, to: &Self, u: f64) -> Self {
        let t = u.clamp(0.0, 1.0);
        let pitch_delta = to.pitch - self.pitch;
        Self {
            yaw: self.yaw + (to.yaw - self.yaw) * t,
            pitch: self.pitch + pitch_delta * t,
            native_pitch: wrap_angle(self.native_pitch + pitch_delta * t),
        }
    }

    /// The wire form: tenths, and the duration in tenths of a second.
    pub fn timed_target(&self, duration: f64) -> Command {
        Command::GimbalTimedTarget {
            yaw_tenth: (self.yaw * 10.0).round() as i32,
            native_pitch_tenth: (self.native_pitch * 10.0).round() as i32,
            duration_tenths: ((duration * 10.0).round() as i64).clamp(1, 255) as u8,
        }
    }
}

fn wrap_angle(deg: f64) -> f64 {
    let mut wrapped = (deg + 180.0) % 360.0;
    if wrapped < 0.0 {
        wrapped += 360.0;
    }
    wrapped - 180.0
}

/// Absolute firmware targets may choose the shortest circular arc. Never send one
/// that could cross the body's missing pan sector.
pub fn can_send(live: &Waypoint, target: &Waypoint) -> bool {
    live.is_reachable()
        && target.is_reachable()
        && ((target.yaw * 10.0).round() / 10.0 - live.yaw).abs() < 180.0
}

/// The session's A·B·C path. C is optional. Durations are per leg.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Program {
    pub a: Option<Waypoint>,
    pub b: Option<Waypoint>,
    pub c: Option<Waypoint>,
    pub duration_ab: f64,
    pub duration_bc: f64,
}

impl Default for Program {
    fn default() -> Self {
        Self {
            a: None,
            b: None,
            c: None,
            duration_ab: 5.0,
            duration_bc: 5.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Leg {
    label: &'static str,
    from: Waypoint,
    to: Waypoint,
    duration: f64,
}

impl Leg {
    fn needs_sub_moves(&self) -> bool {
        self.duration > NATIVE_MAX || (self.to.yaw - self.from.yaw).abs() >= 180.0
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Approach,
    Hold,
    Run,
    Verify,
    Done,
    Failed(String),
}

/// What one tick asks the shell to do.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Output {
    /// A target and how long the camera has to reach it.
    pub target: Option<(Waypoint, f64)>,
    /// Send the native stop: the take is over, well or badly.
    pub stop: bool,
    pub finished: bool,
}

/// One take, from the approach to the final check.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveEngine {
    phase: Phase,
    legs: Vec<Leg>,
    index: usize,
    elapsed: f64,
    approach_targets: Vec<Waypoint>,
    approach_duration: f64,
    pending_command: bool,
    /// Where the current routed leg's next part starts, in leg time.
    next_part_at: f64,
}

impl MoveEngine {
    /// Plans the take from the body's live pose. An error is the operator's message.
    pub fn start(program: &Program, live: Waypoint) -> Result<Self, String> {
        let (Some(a), Some(b)) = (program.a, program.b) else {
            return Err("Set A and B before running".to_string());
        };
        let mut points = vec![a, b, live];
        points.extend(program.c);
        if !points.iter().all(Waypoint::is_reachable) {
            return Err("Set reachable gimbal points again".to_string());
        }
        let mut legs = vec![Leg {
            label: "A→B",
            from: a,
            to: b,
            duration: program.duration_ab,
        }];
        if let Some(c) = program.c {
            legs.push(Leg {
                label: "B→C",
                from: b,
                to: c,
                duration: program.duration_bc,
            });
        }
        if !legs.iter().all(|leg| {
            leg.duration.is_finite()
                && (MIN_DURATION..=MAX_DURATION).contains(&leg.duration)
                && (leg.duration * 10.0 - (leg.duration * 10.0).round()).abs() < 1e-6
        }) {
            return Err("Increase the move duration".to_string());
        }
        // Preparation follows the reachable arc to A in steps under 120°, each at least
        // half a second, then waits at A. It is outside the timed take.
        let steps = ((a.yaw - live.yaw).abs() / 120.0).ceil().max(1.0) as usize;
        let approach_targets = (1..=steps)
            .map(|step| live.lerp(&a, step as f64 / steps as f64))
            .collect();
        let approach_duration =
            (MIN_DURATION).max((live.distance(&a) / steps as f64 / 120.0 * 10.0).ceil() / 10.0);
        let needs_approach = live.distance(&a) > ARRIVE_DEG;
        Ok(Self {
            phase: if needs_approach {
                Phase::Approach
            } else {
                Phase::Hold
            },
            legs,
            index: 0,
            elapsed: 0.0,
            approach_targets,
            approach_duration,
            pending_command: needs_approach,
            next_part_at: 0.0,
        })
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self.phase,
            Phase::Approach | Phase::Hold | Phase::Run | Phase::Verify
        )
    }

    pub fn failure(&self) -> Option<&str> {
        match &self.phase {
            Phase::Failed(reason) => Some(reason),
            _ => None,
        }
    }

    /// What the top bar says while the take runs.
    pub fn readout(&self) -> String {
        match &self.phase {
            Phase::Approach => "MOVE · APPROACHING A".to_string(),
            Phase::Hold => format!(
                "MOVE · HOLD {:.1} s",
                (HOLD_SECONDS - self.elapsed).max(0.0)
            ),
            Phase::Run => {
                let leg = &self.legs[self.index];
                format!(
                    "MOVE · {} {:.1} / {:.1} s",
                    leg.label,
                    self.elapsed.min(leg.duration),
                    leg.duration
                )
            }
            Phase::Verify => "MOVE · CHECKING".to_string(),
            Phase::Done => "MOVE · DONE".to_string(),
            Phase::Failed(reason) => format!("MOVE STOPPED · {reason}"),
        }
    }

    pub fn cancel(&mut self) -> Output {
        self.phase = Phase::Failed("Stopped".to_string());
        Output {
            target: None,
            stop: true,
            finished: true,
        }
    }

    fn stop(&mut self, reason: &str) -> Output {
        self.phase = Phase::Failed(reason.to_string());
        Output {
            target: None,
            stop: true,
            finished: true,
        }
    }

    fn command(&self, live: &Waypoint, target: Waypoint, duration: f64) -> Result<Output, String> {
        if !can_send(live, &target) {
            return Err("Target would cross the gimbal's missing sector".to_string());
        }
        Ok(Output {
            target: Some((target, duration)),
            stop: false,
            finished: false,
        })
    }

    /// One step of `dt` seconds with the body's pose, `telemetry_age` seconds old.
    pub fn tick(&mut self, dt: f64, live: Option<Waypoint>, telemetry_age: f64) -> Output {
        if !self.is_running() {
            return Output::default();
        }
        let Some(live) = live.filter(|live| live.is_reachable()) else {
            return self.stop("Move interrupted — camera feedback lost");
        };
        let timing_ok = dt.is_finite() && dt > 0.0 && dt <= 0.12;
        if !timing_ok || !(0.0..=FRESH_SECONDS).contains(&telemetry_age) {
            return self.stop("Move interrupted — timing or camera feedback lost");
        }
        if self.pending_command {
            self.pending_command = false;
            self.elapsed = 0.0;
            let target = self.approach_targets[0];
            return match self.command(&live, target, self.approach_duration) {
                Ok(output) => output,
                Err(reason) => self.stop(&reason),
            };
        }
        self.elapsed += dt;
        let a = self.legs[0].from;
        match self.phase.clone() {
            Phase::Approach => {
                let target = self.approach_targets[0];
                if self.elapsed >= self.approach_duration && live.distance(&target) <= ARRIVE_DEG {
                    self.approach_targets.remove(0);
                    self.elapsed = 0.0;
                    if let Some(next) = self.approach_targets.first().copied() {
                        return match self.command(&live, next, self.approach_duration) {
                            Ok(output) => output,
                            Err(reason) => self.stop(&reason),
                        };
                    }
                    self.phase = Phase::Hold;
                } else if self.elapsed > self.approach_duration + 0.5 {
                    return self.stop("Camera did not reach A");
                }
                Output::default()
            }
            Phase::Hold => {
                if live.distance(&a) > ARRIVE_DEG {
                    return self.stop("Camera moved before the take");
                }
                if self.elapsed + 1e-9 < HOLD_SECONDS {
                    return Output::default();
                }
                self.phase = Phase::Run;
                self.elapsed = 0.0;
                self.start_leg(&live)
            }
            Phase::Run => {
                let leg = self.legs[self.index];
                if self.elapsed + 1e-9 < leg.duration {
                    if leg.needs_sub_moves() {
                        return self.tick_routed_leg(&live);
                    }
                    return Output::default();
                }
                if leg.needs_sub_moves() && self.next_part_at < leg.duration {
                    return self.stop("Move interrupted — waypoint dispatch was late");
                }
                if self.elapsed - leg.duration > LATE_SECONDS + 1e-9 {
                    return self.stop("Move interrupted — waypoint dispatch was late");
                }
                if self.index + 1 < self.legs.len() {
                    self.index += 1;
                    self.elapsed = 0.0;
                    return self.start_leg(&live);
                }
                self.phase = Phase::Verify;
                self.elapsed = 0.0;
                Output::default()
            }
            Phase::Verify => {
                if self.elapsed < 0.3 {
                    return Output::default();
                }
                if live.distance(&self.legs[self.index].to) > ARRIVE_DEG {
                    return self.stop("Camera missed its final position");
                }
                self.phase = Phase::Done;
                Output {
                    target: None,
                    stop: false,
                    finished: true,
                }
            }
            Phase::Done | Phase::Failed(_) => Output::default(),
        }
    }

    fn start_leg(&mut self, live: &Waypoint) -> Output {
        let leg = self.legs[self.index];
        if leg.needs_sub_moves() {
            self.next_part_at = 0.0;
            return self.send_routed_part(live);
        }
        match self.command(live, leg.to, leg.duration) {
            Ok(output) => output,
            Err(reason) => self.stop(&reason),
        }
    }

    fn tick_routed_leg(&mut self, live: &Waypoint) -> Output {
        if self.elapsed + 1e-9 < self.next_part_at {
            return Output::default();
        }
        if self.elapsed - self.next_part_at > LATE_SECONDS + 1e-9 {
            return self.stop("Move interrupted — waypoint dispatch was late");
        }
        self.send_routed_part(live)
    }

    /// A leg too long or too wide for one native command goes out as parts along the
    /// arc, each part's duration in native tenths and its target on the straight line
    /// at the elapsed fraction of the original leg.
    fn send_routed_part(&mut self, live: &Waypoint) -> Output {
        let leg = self.legs[self.index];
        let parts = (((leg.to.yaw - leg.from.yaw).abs() / 120.0).ceil() as usize)
            .max((leg.duration / NATIVE_MAX).ceil() as usize)
            .max(1);
        let ticks = (leg.duration * 10.0).round() as usize;
        let start = self.next_part_at;
        let mut end_ticks = 0usize;
        for part in 0..parts {
            end_ticks += ticks / parts + usize::from(part < ticks % parts);
            let end = end_ticks as f64 / 10.0;
            if end <= start + 1e-9 {
                continue;
            }
            self.next_part_at = end;
            let target = if part == parts - 1 {
                leg.to
            } else {
                leg.from.lerp(&leg.to, end / leg.duration)
            };
            let duration = (end_ticks as f64 - (start * 10.0).round()) / 10.0;
            return match self.command(live, target, duration) {
                Ok(output) => output,
                Err(reason) => self.stop(&reason),
            };
        }
        self.stop("Move interrupted — waypoint dispatch was late")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(yaw: f64, pitch: f64) -> Waypoint {
        Waypoint {
            yaw,
            pitch,
            native_pitch: pitch,
        }
    }

    fn program(a: Waypoint, b: Waypoint, c: Option<Waypoint>, seconds: f64) -> Program {
        Program {
            a: Some(a),
            b: Some(b),
            c,
            duration_ab: seconds,
            duration_bc: seconds,
        }
    }

    /// Drives the engine with a body that lands wherever it was sent, on time.
    fn run(engine: &mut MoveEngine, mut live: Waypoint, seconds: f64) -> Vec<(Waypoint, f64)> {
        let mut sent = Vec::new();
        let mut t = 0.0;
        while t < seconds && engine.is_running() {
            let out = engine.tick(0.04, Some(live), 0.05);
            if let Some((target, duration)) = out.target {
                sent.push((target, duration));
                live = target;
            }
            t += 0.04;
        }
        sent
    }

    #[test]
    fn raw_yaw_unwraps_onto_the_reachable_arc() {
        assert_eq!(Waypoint::unwrap_yaw(10.0), 10.0);
        assert_eq!(Waypoint::unwrap_yaw(-135.0), 225.0);
        assert_eq!(Waypoint::unwrap_yaw(-48.0), -48.0);
        assert!(at(225.0, 0.0).is_reachable());
        assert!(!at(-60.0, 0.0).is_reachable());
        assert!(!at(0.0, 80.0).is_reachable());
    }

    #[test]
    fn a_take_approaches_holds_runs_and_verifies() {
        let a = at(10.0, 0.0);
        let b = at(50.0, 10.0);
        let mut engine = MoveEngine::start(&program(a, b, None, 4.0), at(0.0, 0.0)).unwrap();
        let sent = run(&mut engine, at(0.0, 0.0), 10.0);
        assert_eq!(sent.len(), 2, "one approach command, one leg: {sent:?}");
        assert_eq!(sent[0].0, a);
        assert!(
            (sent[0].1 - 0.5).abs() < 1e-9,
            "a short approach takes the floor"
        );
        assert_eq!(sent[1], (b, 4.0));
        assert_eq!(engine.readout(), "MOVE · DONE");
        assert!(!engine.is_running());
    }

    #[test]
    fn a_far_approach_is_taken_in_steps_under_120_degrees() {
        let a = at(220.0, 0.0);
        let mut engine =
            MoveEngine::start(&program(a, at(200.0, 0.0), None, 2.0), at(0.0, 0.0)).unwrap();
        let sent = run(&mut engine, at(0.0, 0.0), 12.0);
        let approach: Vec<f64> = sent.iter().take(2).map(|(w, _)| w.yaw).collect();
        assert_eq!(approach, [110.0, 220.0]);
        assert!(
            sent[0].1 >= 110.0 / 120.0,
            "each step allows 120°/s: {}",
            sent[0].1
        );
        assert_eq!(sent.last().unwrap().0.yaw, 200.0);
    }

    #[test]
    fn a_wide_leg_goes_out_as_native_parts_along_the_arc() {
        let a = at(0.0, 0.0);
        let b = at(200.0, 0.0);
        let mut engine = MoveEngine::start(&program(a, b, None, 10.0), a).unwrap();
        let sent = run(&mut engine, a, 20.0);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0].0.yaw, 100.0);
        assert_eq!(sent[0].1, 5.0);
        assert_eq!(sent[1].0.yaw, 200.0);
        assert_eq!(sent[1].1, 5.0);
        assert_eq!(engine.readout(), "MOVE · DONE");
    }

    #[test]
    fn a_long_leg_is_split_at_the_native_limit() {
        let a = at(0.0, 0.0);
        let b = at(30.0, 0.0);
        let mut engine = MoveEngine::start(&program(a, b, None, 40.0), a).unwrap();
        let sent = run(&mut engine, a, 60.0);
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|(_, d)| *d <= NATIVE_MAX));
        assert!((sent[0].1 + sent[1].1 - 40.0).abs() < 1e-9);
    }

    #[test]
    fn stale_feedback_or_a_body_that_never_arrives_stops_the_take() {
        let a = at(10.0, 0.0);
        let mut engine =
            MoveEngine::start(&program(a, at(20.0, 0.0), None, 2.0), at(0.0, 0.0)).unwrap();
        assert!(engine.tick(0.04, Some(at(0.0, 0.0)), 0.05).target.is_some());
        let out = engine.tick(0.04, Some(at(0.0, 0.0)), 0.5);
        assert!(out.stop);
        assert!(engine.failure().unwrap().contains("feedback"));

        let mut engine =
            MoveEngine::start(&program(a, at(20.0, 0.0), None, 2.0), at(0.0, 0.0)).unwrap();
        engine.tick(0.04, Some(at(0.0, 0.0)), 0.05);
        let mut out = Output::default();
        for _ in 0..40 {
            out = engine.tick(0.04, Some(at(0.0, 0.0)), 0.05);
            if out.stop {
                break;
            }
        }
        assert!(out.stop);
        assert_eq!(engine.failure(), Some("Camera did not reach A"));
    }

    #[test]
    fn a_target_half_a_turn_away_is_never_sent() {
        assert!(!can_send(&at(0.0, 0.0), &at(180.0, 0.0)));
        assert!(can_send(&at(0.0, 0.0), &at(179.9, 0.0)));
        assert!(!can_send(&at(-60.0, 0.0), &at(0.0, 0.0)));
        assert_eq!(
            at(12.34, -5.0).timed_target(2.0),
            Command::GimbalTimedTarget {
                yaw_tenth: 123,
                native_pitch_tenth: -50,
                duration_tenths: 20
            }
        );
    }

    #[test]
    fn a_program_needs_a_and_b_and_a_sane_duration() {
        let live = at(0.0, 0.0);
        assert!(MoveEngine::start(&Program::default(), live).is_err());
        let mut bad = program(at(0.0, 0.0), at(10.0, 0.0), None, 0.3);
        assert_eq!(
            MoveEngine::start(&bad, live).unwrap_err(),
            "Increase the move duration"
        );
        bad.duration_ab = 2.0;
        bad.b = Some(at(300.0, 0.0));
        assert_eq!(
            MoveEngine::start(&bad, live).unwrap_err(),
            "Set reachable gimbal points again"
        );
    }
}
