//! The camera's own network, and whether this machine is on it.
//!
//! A socket that is not on-path looks exactly like a camera that is not answering, so
//! these gates decide what an operator gets told. They are the core's, not guesses.

use std::ffi::CString;

use opc_core_sys as sys;

/// The camera's fixed address.
pub fn host() -> String {
    let needed = {
        // Safety: probing with a null destination only reports the size.
        unsafe { sys::opc_softap_host(std::ptr::null_mut(), 0) }
    };
    if needed <= 0 {
        return String::new();
    }
    let mut bytes = vec![0u8; needed as usize];
    // Safety: `bytes` has exactly the capacity the core asked for.
    let written = unsafe { sys::opc_softap_host(bytes.as_mut_ptr(), bytes.len()) };
    if written != needed {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The camera's port. It is the remote only — never bind it locally.
pub fn remote_port() -> u16 {
    // Safety: no arguments, no allocation.
    unsafe { sys::opc_softap_remote_port() }
}

/// True when the datalink may bind this local port.
///
/// Binding the camera's own port locally accepted telemetry and dropped every video
/// packet on a Samsung. Only an ephemeral port is correct.
pub fn may_bind_local_port(port: u16) -> bool {
    // Safety: no allocation.
    unsafe { sys::opc_softap_may_bind_local_port(port) == 1 }
}

/// True when this address means the machine is associated with the camera.
pub fn is_associated(ipv4: &str) -> bool {
    let Ok(text) = CString::new(ipv4) else {
        return false;
    };
    // Safety: `text` outlives the call.
    unsafe { sys::opc_softap_is_associated(text.as_ptr()) == 1 }
}

/// True when any of these local addresses puts the machine on the camera's path.
pub fn is_path_ready(addresses: &[String]) -> bool {
    let Ok(joined) = CString::new(addresses.join("\n")) else {
        return false;
    };
    // Safety: `joined` outlives the call.
    unsafe { sys::opc_softap_is_path_ready(joined.as_ptr()) == 1 }
}

/// True when this SSID looks like an Osmo's own access point.
pub fn is_camera_ssid(ssid: &str) -> bool {
    let Ok(text) = CString::new(ssid) else {
        return false;
    };
    // Safety: `text` outlives the call.
    unsafe { sys::opc_softap_is_camera_ssid(text.as_ptr()) == 1 }
}
