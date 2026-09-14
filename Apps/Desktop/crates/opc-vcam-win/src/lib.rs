//! `opc_vcam_win.dll`: the viewfinder as a camera on Windows 11.
//!
//! Windows 11 22H2 lets an app register a *virtual camera* whose media source is a COM
//! object the Windows Camera Frame Server loads into its own service process. This DLL
//! is that source. The viewfinder registers the camera for its session with
//! `MFCreateVirtualCamera` (see `opc_vcam::win`) and serves NV12 frames over the named
//! pipe in [`opc_vcam::wire`]; the source reads them and hands them to whichever app
//! opened the camera, at 30 frames a second, or a black frame while nothing is coming.
//!
//! Registration is one `regsvr32 opc_vcam_win.dll` as administrator, since the Frame
//! Server activates the class from the machine hive. Nothing is signed and nothing runs
//! in the kernel.
//!
//! On any other platform this crate builds to nothing.

#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
mod bridge;
#[cfg(windows)]
mod dll;
#[cfg(windows)]
mod source;
#[cfg(windows)]
mod stream;

#[cfg(windows)]
pub use dll::{
    DllCanUnloadNow, DllGetClassObject, DllMain, DllRegisterServer, DllUnregisterServer,
};

/// The source's class id, as the viewfinder names it to `MFCreateVirtualCamera`.
pub const CLSID_TEXT: &str = "{6f0b4c1e-8d7a-4f3b-9c2e-5a1d7e9b3c40}";
/// The name apps show.
pub const FRIENDLY_NAME: &str = "OpenPocketCine";
/// Frames a second the source promises.
pub const FRAME_RATE: u32 = 30;

#[cfg(test)]
mod tests {
    #[test]
    fn the_class_id_is_the_one_the_viewfinder_asks_for() {
        assert_eq!(super::CLSID_TEXT, opc_vcam::wire::SOURCE_CLSID);
    }
}
