//! The Bluetooth side of pairing.
//!
//! The camera exposes one service with a notify characteristic and a write
//! characteristic; DUML frames go out on the write one and come back, split across
//! several notifications, on the notify one. Reassembling those is the core's job, not a
//! guess about MTUs.
//!
//! [`BleTransport`] is what a platform stack has to provide. `btleplug` covers Windows,
//! macOS and Linux behind one API and is the intended implementation; it is not written
//! yet, and nothing here pretends otherwise.

use std::ffi::c_void;
use std::io;

use opc_core_sys as sys;

use crate::packed::{self, DumlFrame};

/// The identifiers a platform stack needs to find the camera's service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GattMap {
    pub service: String,
    /// Notifications arrive here. Arming pairing is also a write to this one.
    pub notify: String,
    /// Commands are written here.
    pub write: String,
    /// The client-configuration descriptor that turns notifications on.
    pub configuration: String,
}

impl GattMap {
    /// Read from the core rather than written down in the shell.
    pub fn from_core() -> Option<Self> {
        // Safety: probing with a null destination only reports the size.
        let needed = unsafe { sys::opc_ble_gatt_uuids(std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return None;
        }
        let mut bytes = vec![0u8; needed as usize];
        // Safety: `bytes` has exactly the capacity the core asked for.
        let written = unsafe { sys::opc_ble_gatt_uuids(bytes.as_mut_ptr(), bytes.len()) };
        if written != needed {
            return None;
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut parts = text.split('\n');
        Some(Self {
            service: parts.next()?.to_string(),
            notify: parts.next()?.to_string(),
            write: parts.next()?.to_string(),
            configuration: parts.next()?.to_string(),
        })
    }
}

/// What a camera's advertisement says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advert {
    /// `None` when the advert does not carry one; the advertised name is the fallback.
    pub model_id: Option<i32>,
    pub new_format: bool,
    pub raw_product_type: Option<i32>,
}

impl Advert {
    pub fn decode(payload: &[u8]) -> Option<Self> {
        let (mut model, mut new_format, mut raw) = (0i32, 0i32, 0i32);
        // Safety: `payload` outlives the call and all three outputs are live.
        let status = unsafe {
            sys::opc_ble_advert_decode(
                payload.as_ptr(),
                payload.len(),
                &mut model,
                &mut new_format,
                &mut raw,
            )
        };
        (status == 1).then_some(Self {
            model_id: (model >= 0).then_some(model),
            new_format: new_format != 0,
            raw_product_type: (raw >= 0).then_some(raw),
        })
    }
}

/// Turns notifications back into whole frames.
///
/// A notification is not a frame: the camera splits replies across several, and a reader
/// that treats each one as complete sees truncated payloads rather than an error.
#[derive(Debug)]
pub struct NotificationAssembler {
    handle: *mut c_void,
}

impl NotificationAssembler {
    pub fn new() -> Self {
        // Safety: the core returns a retained handle released in `Drop`.
        Self {
            handle: unsafe { sys::opc_ble_assembler_create() },
        }
    }

    /// Appends one notification, returning any frames it completed.
    pub fn append(&mut self, bytes: &[u8]) -> Vec<DumlFrame> {
        // Safety: `bytes` outlives the call; a null destination only reports the size.
        let needed = unsafe {
            sys::opc_ble_assembler_append(
                self.handle,
                bytes.as_ptr(),
                bytes.len(),
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 0 {
            return Vec::new();
        }
        let mut blob = vec![0u8; needed as usize];
        // Fetched rather than appended again: a second append would consume the *next*
        // notification and lose these frames.
        // Safety: `blob` has exactly the capacity the core asked for.
        let written =
            unsafe { sys::opc_ble_assembler_take(self.handle, blob.as_mut_ptr(), blob.len()) };
        if written <= 0 {
            return Vec::new();
        }
        packed::split(&blob)
            .into_iter()
            .filter_map(DumlFrame::parse)
            .collect()
    }
}

impl Default for NotificationAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NotificationAssembler {
    fn drop(&mut self) {
        // Safety: retained by `opc_ble_assembler_create` and released exactly once.
        unsafe { sys::opc_ble_assembler_destroy(self.handle) }
    }
}

// Plain heap state with no thread affinity; `&mut self` gates every mutating call.
unsafe impl Send for NotificationAssembler {}

/// A camera seen in a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub name: String,
    /// Whatever the platform uses to address this device again.
    pub address: String,
    pub advert: Option<Advert>,
}

/// What a platform Bluetooth stack has to provide.
///
/// Deliberately small: scan, connect, write, and hand over notifications. Everything
/// about *what* to write is [`crate::Pairing`]'s, and everything about what the bytes
/// mean is the core's.
pub trait BleTransport {
    /// Cameras advertising the service, gathered for roughly `seconds`.
    fn scan(&mut self, seconds: f64) -> io::Result<Vec<Discovered>>;
    /// Connects and turns notifications on.
    fn connect(&mut self, address: &str) -> io::Result<()>;
    /// Writes one encoded DUML frame to the command characteristic.
    fn write_frame(&mut self, frame: &[u8]) -> io::Result<()>;
    /// Notifications received since the last call, each still a fragment.
    fn take_notifications(&mut self) -> Vec<Vec<u8>>;
    fn disconnect(&mut self) -> io::Result<()>;
}
