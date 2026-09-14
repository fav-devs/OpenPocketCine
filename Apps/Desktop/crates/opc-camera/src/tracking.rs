//! What the body says about the subject it is following, read by the core.

/// A `0x02/0xA5` poll reply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackingPoll {
    /// Nothing is being followed.
    Idle,
    /// Locked on; with the subject's box as picture fractions when the body sent one.
    Locked(Option<(f32, f32, f32, f32)>),
}

/// Reads a poll reply through the core. `None` for a payload it does not recognise,
/// or without the core linked.
#[cfg(opc_core_linked)]
pub fn tracking_poll(payload: &[u8]) -> Option<TrackingPoll> {
    use opc_core_sys as sys;
    let mut out = [0.0_f32; 4];
    // Safety: the slice outlives the call and `out` has the four slots the core writes.
    let code = unsafe { sys::opc_tracking_poll(payload.as_ptr(), payload.len(), out.as_mut_ptr()) };
    match code {
        sys::OPC_TRACKING_IDLE => Some(TrackingPoll::Idle),
        sys::OPC_TRACKING_LOCKED => Some(TrackingPoll::Locked(None)),
        sys::OPC_TRACKING_LOCKED_BOX => {
            Some(TrackingPoll::Locked(Some((out[0], out[1], out[2], out[3]))))
        }
        _ => None,
    }
}

#[cfg(not(opc_core_linked))]
pub fn tracking_poll(_payload: &[u8]) -> Option<TrackingPoll> {
    None
}

/// Whether this body takes the tap-to-focus burst. Every Pocket does; the Nano does
/// not. Without the core linked every body is assumed to.
#[cfg(opc_core_linked)]
pub fn supports_tap_focus(model_id: i32) -> bool {
    // Safety: a plain value in.
    unsafe { opc_core_sys::opc_model_supports_tap_focus(model_id) != 0 }
}

#[cfg(not(opc_core_linked))]
pub fn supports_tap_focus(_model_id: i32) -> bool {
    true
}
