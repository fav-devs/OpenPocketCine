//! Video packets to access units.
//!
//! The camera splits an access unit across several pktType-`0x02` datagrams. Putting
//! them back together — and knowing when one is complete — is the core's
//! `HevcDepacketizer`; this is a handle onto it.

use std::ffi::c_void;

use opc_core_sys as sys;

/// Reassembles one session's video stream.
#[derive(Debug)]
pub struct Depacketizer {
    handle: *mut c_void,
}

impl Depacketizer {
    pub fn new() -> Self {
        // Safety: the core returns a retained handle released in `Drop`.
        Self {
            handle: unsafe { sys::opc_depacketizer_create() },
        }
    }

    /// Feeds one whole pktType-`0x02` datagram, transport header included.
    ///
    /// Not the payload: the core reads the packet type at byte 6 and the fragment index
    /// at bytes 16 to 18, and takes the encoded body from byte 20.
    ///
    /// Returns an access unit when this packet completed one — which is usually the
    /// packet that *starts the next* frame, since that is when the previous one is known
    /// to be whole.
    pub fn feed(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        // Safety: `payload` outlives the call; a null destination only reports the size.
        let needed = unsafe {
            sys::opc_depacketizer_feed(
                self.handle,
                payload.as_ptr(),
                payload.len(),
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 0 {
            return None;
        }
        let mut unit = vec![0u8; needed as usize];
        // Safety: `unit` has exactly the capacity the core asked for.
        let written =
            unsafe { sys::opc_depacketizer_take(self.handle, unit.as_mut_ptr(), unit.len()) };
        if written != needed {
            return None;
        }
        Some(unit)
    }

    /// Drops partial state after a rebuild. The acknowledgement cursor is not this
    /// handle's and must survive.
    pub fn reset(&mut self) {
        // Safety: the handle is live for the lifetime of `self`.
        unsafe { sys::opc_depacketizer_reset(self.handle) }
    }

    /// How many access units were abandoned incomplete — a packet-loss signal.
    pub fn dropped(&self) -> i32 {
        // Safety: the handle is live for the lifetime of `self`.
        unsafe { sys::opc_depacketizer_dropped(self.handle) }
    }
}

impl Default for Depacketizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Depacketizer {
    fn drop(&mut self) {
        // Safety: retained by `opc_depacketizer_create` and released exactly once.
        unsafe { sys::opc_depacketizer_destroy(self.handle) }
    }
}

// Plain heap state with no thread affinity; `&mut self` gates every mutating call.
unsafe impl Send for Depacketizer {}
