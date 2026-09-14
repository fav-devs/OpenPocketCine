//! Zoom the way the body allows it: the chip stops per model, format and shooting
//! mode, and the D-Log2 hop.
//!
//! Both rules are the core's (`CameraModel.activeZoomStops`, `CamFov`), reached through
//! the facade. D-Log2 rejects every zoom SET, so a zoom off 1× first hops the colour
//! mode to D-Log, holds the write until the body reports the hop, and puts D-Log2 back
//! when the zoom is parked at 1× again — never while rolling, because the body will not
//! change colour then. Without the core linked a stand-in answers with the Pocket 4 Pro
//! stops and the same D-Log2 rule, so the shell can be exercised on any machine.

use opc_camera::Command;

/// What a zoom write needs before it can go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hop {
    /// Nothing; send it.
    None,
    /// Set this colour mode first and hold the write until the body reports it.
    Color(u8),
    /// The body is rolling and will not change colour: the zoom cannot happen.
    Blocked,
}

/// The core's answers about zoom.
pub trait ZoomPolicy: std::fmt::Debug {
    fn stops(&self, model_id: i32, resolution: Option<u8>, shooting_mode: Option<i32>) -> Vec<f64>;
    fn hop(&self, factor: f64, color_mode: Option<u8>, recording: bool) -> Hop;
    fn restore_dlog2(&self, factor: f64) -> bool;
}

/// The core, through the facade.
#[cfg(opc_core_linked)]
#[derive(Debug, Default)]
pub struct CorePolicy;

#[cfg(opc_core_linked)]
impl ZoomPolicy for CorePolicy {
    fn stops(&self, model_id: i32, resolution: Option<u8>, shooting_mode: Option<i32>) -> Vec<f64> {
        let mut out = [0.0_f64; 8];
        // Safety: `out` has the capacity handed over.
        let count = unsafe {
            opc_core_sys::opc_zoom_stops(
                model_id,
                resolution.map_or(-1, i32::from),
                shooting_mode.unwrap_or(-1),
                out.as_mut_ptr(),
                out.len(),
            )
        };
        let count = usize::try_from(count).unwrap_or(0).min(out.len());
        out[..count].to_vec()
    }

    fn hop(&self, factor: f64, color_mode: Option<u8>, recording: bool) -> Hop {
        let mut mode = 0_i32;
        // Safety: `mode` outlives the call.
        let code = unsafe {
            opc_core_sys::opc_zoom_hop(
                factor,
                color_mode.map_or(-1, i32::from),
                i32::from(recording),
                &mut mode,
            )
        };
        match code {
            opc_core_sys::OPC_ZOOM_HOP_BLOCKED => Hop::Blocked,
            opc_core_sys::OPC_ZOOM_HOP_COLOR => Hop::Color(u8::try_from(mode).unwrap_or(0x17)),
            _ => Hop::None,
        }
    }

    fn restore_dlog2(&self, factor: f64) -> bool {
        // Safety: a plain value in.
        unsafe { opc_core_sys::opc_zoom_restore_dlog2(factor) != 0 }
    }
}

/// A stand-in for machines without the core: the Pocket 4 Pro's stops and the D-Log2
/// rule as the phones state it. Not a second copy of the per-body table.
#[derive(Debug, Default)]
pub struct Assumed;

const DLOG2: u8 = 0x41;
const DLOG: u8 = 0x17;

impl ZoomPolicy for Assumed {
    fn stops(
        &self,
        _model_id: i32,
        _resolution: Option<u8>,
        _shooting_mode: Option<i32>,
    ) -> Vec<f64> {
        vec![1.0, 3.0, 6.0, 12.0]
    }

    fn hop(&self, factor: f64, color_mode: Option<u8>, recording: bool) -> Hop {
        if color_mode != Some(DLOG2) || factor <= 1.0 {
            return Hop::None;
        }
        if recording {
            Hop::Blocked
        } else {
            Hop::Color(DLOG)
        }
    }

    fn restore_dlog2(&self, factor: f64) -> bool {
        factor <= 1.05
    }
}

/// The policy this build has.
pub fn policy() -> Box<dyn ZoomPolicy> {
    #[cfg(opc_core_linked)]
    {
        Box::new(CorePolicy)
    }
    #[cfg(not(opc_core_linked))]
    {
        Box::new(Assumed)
    }
}

/// The chip stops for the body as it is set up right now.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomRules {
    stops: Vec<f64>,
    key: (i32, Option<u8>, Option<i32>),
}

impl Default for ZoomRules {
    fn default() -> Self {
        Self {
            stops: vec![1.0],
            key: (i32::MIN, None, None),
        }
    }
}

impl ZoomRules {
    /// Re-reads the stops when the body, format or shooting mode moved. True on a change.
    pub fn refresh(
        &mut self,
        policy: &dyn ZoomPolicy,
        model_id: i32,
        resolution: Option<u8>,
        shooting_mode: Option<i32>,
    ) -> bool {
        let key = (model_id, resolution, shooting_mode);
        if key == self.key {
            return false;
        }
        self.key = key;
        let mut stops = policy.stops(model_id, resolution, shooting_mode);
        if stops.is_empty() {
            stops.push(1.0);
        }
        let changed = stops != self.stops;
        self.stops = stops;
        changed
    }

    pub fn stops(&self) -> &[f64] {
        &self.stops
    }

    pub fn max(&self) -> f64 {
        self.stops.last().copied().unwrap_or(1.0)
    }

    /// The body has more than 1×.
    pub fn can_zoom(&self) -> bool {
        self.max() > 1.0
    }

    /// The next stop up, or the first when already at the top (`CamFov.nextJump`).
    pub fn next(&self, from: f64) -> f64 {
        self.stops
            .iter()
            .copied()
            .find(|stop| from < stop - 0.05)
            .unwrap_or(self.stops[0])
    }

    /// The next stop down; stays on the wide end (`CamFov.previousJump`).
    pub fn previous(&self, from: f64) -> f64 {
        self.stops
            .iter()
            .rev()
            .copied()
            .find(|stop| from > stop + 0.05)
            .unwrap_or(self.stops[0])
    }

    pub fn clamp(&self, factor: f64) -> f64 {
        factor.clamp(1.0, self.max())
    }
}

/// One zoom request from the operator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomWrite {
    /// The dial: coalesced, not retransmitted.
    Slider(f64),
    /// A key or a stop: urgent, retransmitted.
    Jump(f64),
}

impl ZoomWrite {
    pub fn factor(self) -> f64 {
        match self {
            Self::Slider(factor) | Self::Jump(factor) => factor,
        }
    }

    fn command(self) -> Command {
        match self {
            Self::Slider(factor) => Command::ZoomFactor(factor),
            Self::Jump(factor) => Command::ZoomJump(factor),
        }
    }
}

/// How long to wait for the body to report the colour hop before giving up on it.
const HOP_TIMEOUT: f64 = 2.0;

/// What the operator should read about a zoom that could not go as asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomNote {
    /// "D-Log — D-Log2 cannot zoom": the hop went out, the zoom waits.
    Hopping,
    /// The body is rolling in D-Log2 and stays at 1×.
    LockedWhileRecording,
    /// The hop was never reported; the zoom is dropped.
    HopTimedOut,
    /// Parked back at 1×, D-Log2 put back.
    Restored,
}

impl ZoomNote {
    pub fn text(&self) -> &'static str {
        match self {
            Self::Hopping => "D-LOG · D-LOG2 CANNOT ZOOM",
            Self::LockedWhileRecording => "ZOOM LOCKED · D-LOG2 WHILE ROLLING",
            Self::HopTimedOut => "ZOOM DROPPED · COLOUR DID NOT HOP",
            Self::Restored => "ZOOM 1× · D-LOG2",
        }
    }
}

/// The D-Log2 hop, held until the body reports it.
#[derive(Debug, Default)]
pub struct ZoomHop {
    /// The write waiting on the hop, the mode it waits for, and when to give up.
    pending: Option<(ZoomWrite, u8, f64)>,
    /// A hop happened, so parking at 1× should put D-Log2 back.
    restore_on_wide: bool,
}

impl ZoomHop {
    /// The commands for a zoom request, and a note when it could not go as asked.
    pub fn request(
        &mut self,
        write: ZoomWrite,
        policy: &dyn ZoomPolicy,
        color_mode: Option<u8>,
        recording: bool,
        model_id: i32,
        now: f64,
    ) -> (Vec<Command>, Option<ZoomNote>) {
        let factor = write.factor();
        match policy.hop(factor, color_mode, recording) {
            Hop::Blocked => (Vec::new(), Some(ZoomNote::LockedWhileRecording)),
            Hop::Color(mode) => {
                let already = self
                    .pending
                    .is_some_and(|(_, wanted, deadline)| wanted == mode && now < deadline);
                self.pending = Some((write, mode, now + HOP_TIMEOUT));
                self.restore_on_wide = true;
                if already {
                    (Vec::new(), None)
                } else {
                    (
                        vec![Command::SetColorMode { mode, model_id }],
                        Some(ZoomNote::Hopping),
                    )
                }
            }
            Hop::None => {
                self.pending = None;
                let mut out = vec![write.command()];
                let mut note = None;
                if self.restore_on_wide && policy.restore_dlog2(factor) && !recording {
                    self.restore_on_wide = false;
                    out.push(Command::SetColorMode {
                        mode: DLOG2,
                        model_id,
                    });
                    note = Some(ZoomNote::Restored);
                }
                (out, note)
            }
        }
    }

    /// The body reported its colour mode: releases the held zoom when the hop landed,
    /// or drops it when the hop timed out.
    pub fn status(
        &mut self,
        color_mode: Option<u8>,
        now: f64,
    ) -> (Option<Command>, Option<ZoomNote>) {
        let Some((write, wanted, deadline)) = self.pending else {
            return (None, None);
        };
        if color_mode == Some(wanted) {
            self.pending = None;
            return (Some(write.command()), None);
        }
        if now >= deadline {
            self.pending = None;
            return (None, Some(ZoomNote::HopTimedOut));
        }
        (None, None)
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stops_cycle_up_and_stay_on_the_wide_end_going_down() {
        let mut rules = ZoomRules::default();
        assert!(!rules.can_zoom(), "nothing known yet is 1× only");
        assert!(rules.refresh(&Assumed, 0x20, None, None));
        assert_eq!(rules.next(1.0), 3.0);
        assert_eq!(rules.next(4.0), 6.0);
        assert_eq!(rules.next(12.0), 1.0, "wraps from the top");
        assert_eq!(rules.previous(12.0), 6.0);
        assert_eq!(rules.previous(1.0), 1.0);
        assert_eq!(rules.clamp(40.0), 12.0);
        assert!(
            !rules.refresh(&Assumed, 0x20, None, None),
            "same body, no change"
        );
    }

    #[test]
    fn dlog2_hops_to_dlog_first_and_comes_back_at_one_x() {
        let mut hop = ZoomHop::default();
        let (sent, note) = hop.request(ZoomWrite::Jump(3.0), &Assumed, Some(DLOG2), false, 7, 0.0);
        assert_eq!(
            sent,
            [Command::SetColorMode {
                mode: DLOG,
                model_id: 7
            }]
        );
        assert_eq!(note, Some(ZoomNote::Hopping));
        assert!(hop.is_pending());
        // A second request while the hop is out does not send a second hop.
        let (sent, _) = hop.request(ZoomWrite::Jump(6.0), &Assumed, Some(DLOG2), false, 7, 0.5);
        assert!(sent.is_empty());
        // The body reports D-Log: the newest held zoom goes.
        assert_eq!(hop.status(Some(DLOG2), 0.6), (None, None));
        assert_eq!(
            hop.status(Some(DLOG), 0.7),
            (Some(Command::ZoomJump(6.0)), None)
        );
        // Parking at 1× restores D-Log2.
        let (sent, note) = hop.request(ZoomWrite::Jump(1.0), &Assumed, Some(DLOG), false, 7, 1.0);
        assert_eq!(
            sent,
            [
                Command::ZoomJump(1.0),
                Command::SetColorMode {
                    mode: DLOG2,
                    model_id: 7
                }
            ]
        );
        assert_eq!(note, Some(ZoomNote::Restored));
    }

    #[test]
    fn rolling_in_dlog2_locks_the_zoom_and_a_lost_hop_drops_it() {
        let mut hop = ZoomHop::default();
        let (sent, note) = hop.request(ZoomWrite::Slider(2.0), &Assumed, Some(DLOG2), true, 7, 0.0);
        assert!(sent.is_empty());
        assert_eq!(note, Some(ZoomNote::LockedWhileRecording));
        hop.request(ZoomWrite::Slider(2.0), &Assumed, Some(DLOG2), false, 7, 1.0);
        assert_eq!(
            hop.status(Some(DLOG2), 3.5),
            (None, Some(ZoomNote::HopTimedOut))
        );
        assert!(!hop.is_pending());
    }
}
