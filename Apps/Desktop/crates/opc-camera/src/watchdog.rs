//! The recover ladder for a feed that stopped.
//!
//! A frozen feed does not announce itself — the socket stays open and telemetry keeps
//! arriving. `FeedWatchdog` in the core decides what to do and in what order; this is a
//! handle onto it.

use std::ffi::c_void;

use opc_core_sys::{self as sys, OpcWatchdogSnapshot};

/// A rung of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Ask for live view again. Enables after the first picture are the watchdog's
    /// alone — the connect path sends exactly one.
    ResendEnable,
    /// The decoder is wedged; the shell rebuilds it.
    RebuildDecoder,
    /// Tear the datalink down and open a new session.
    ReopenDatalink,
    /// Start over from Bluetooth.
    FullRejoin,
}

/// Drives the ladder.
#[derive(Debug)]
pub struct Watchdog {
    handle: *mut c_void,
}

impl Watchdog {
    pub fn new() -> Self {
        // Safety: the core returns a retained handle released in `Drop`.
        Self {
            handle: unsafe { sys::opc_watchdog_create() },
        }
    }

    /// How long without a picture counts as a stall.
    pub fn stall_threshold() -> f64 {
        // Safety: no arguments, no allocation.
        unsafe { sys::opc_watchdog_stall_threshold() }
    }

    /// Folds one observation in and returns the rung to act on, if any.
    pub fn tick(&mut self, snapshot: &OpcWatchdogSnapshot) -> Option<Recovery> {
        // Safety: `snapshot` outlives the call and the handle is live for `self`.
        match unsafe { sys::opc_watchdog_tick(self.handle, snapshot) } {
            sys::OPC_WATCHDOG_RESEND_ENABLE => Some(Recovery::ResendEnable),
            sys::OPC_WATCHDOG_REBUILD_DECODER => Some(Recovery::RebuildDecoder),
            sys::OPC_WATCHDOG_REOPEN_DATALINK => Some(Recovery::ReopenDatalink),
            sys::OPC_WATCHDOG_FULL_REJOIN => Some(Recovery::FullRejoin),
            _ => None,
        }
    }

    /// Which rung the ladder is resting on, for diagnostics.
    pub fn stage(&self) -> String {
        // Safety: probing with a null destination only reports the size.
        let needed = unsafe { sys::opc_watchdog_stage(self.handle, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return String::new();
        }
        let mut bytes = vec![0u8; needed as usize];
        // Safety: `bytes` has exactly the capacity the core asked for.
        let written =
            unsafe { sys::opc_watchdog_stage(self.handle, bytes.as_mut_ptr(), bytes.len()) };
        if written != needed {
            return String::new();
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Default for Watchdog {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        // Safety: retained by `opc_watchdog_create` and released exactly once.
        unsafe { sys::opc_watchdog_destroy(self.handle) }
    }
}

// Plain heap state with no thread affinity; `&mut self` gates every mutating call.
unsafe impl Send for Watchdog {}
