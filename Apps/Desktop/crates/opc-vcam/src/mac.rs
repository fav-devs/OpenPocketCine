//! The macOS camera: the OpenPocketCine camera extension's sink stream, written
//! through the Swift facade (`opc_vcam_mac_*`). The extension itself is installed once
//! from the OpenPocketCine Camera app (`Apps/Desktop/macos`).

use crate::wire::rgba_to_nv12;
use crate::{Error, Frame, Sink};

/// The open sink stream.
#[derive(Debug)]
pub struct MacCamSink {
    width: u32,
    height: u32,
}

#[cfg(opc_core_linked)]
impl MacCamSink {
    pub fn open(width: u32, height: u32) -> Result<Self, Error> {
        // Safety: plain values in; the facade keeps its own state.
        let rc = unsafe { opc_core_sys::opc_vcam_mac_open(width as i32, height as i32) };
        match rc {
            0 => Ok(Self { width, height }),
            -1 => Err(Error::Frame(
                "no OpenPocketCine camera — install the OpenPocketCine Camera app and enable its extension".into(),
            )),
            -2 => Err(Error::Frame("the camera extension's sink stream would not start".into())),
            other => Err(Error::Frame(format!("the camera could not open ({other})"))),
        }
    }
}

#[cfg(not(opc_core_linked))]
impl MacCamSink {
    pub fn open(_width: u32, _height: u32) -> Result<Self, Error> {
        Err(Error::Frame(
            "this build has no Swift core, which the macOS camera goes through".into(),
        ))
    }
}

impl Sink for MacCamSink {
    fn describe(&self) -> String {
        format!(
            "Camera · OpenPocketCine (camera extension) · {}×{}",
            self.width, self.height
        )
    }

    fn push(&mut self, frame: &Frame) -> Result<(), Error> {
        if frame.width != self.width || frame.height != self.height {
            return Err(Error::Frame(format!(
                "frame is {}×{}, the camera was set to {}×{}",
                frame.width, frame.height, self.width, self.height
            )));
        }
        let nv12 = rgba_to_nv12(frame.width, frame.height, &frame.rgba);
        push_nv12(&nv12)
    }
}

#[cfg(opc_core_linked)]
fn push_nv12(nv12: &[u8]) -> Result<(), Error> {
    // Safety: the slice outlives the call; the facade copies it into a pixel buffer.
    let rc = unsafe { opc_core_sys::opc_vcam_mac_push(nv12.as_ptr(), nv12.len()) };
    if rc < 0 {
        return Err(Error::Frame(format!("the camera refused a frame ({rc})")));
    }
    Ok(())
}

#[cfg(not(opc_core_linked))]
fn push_nv12(_nv12: &[u8]) -> Result<(), Error> {
    Ok(())
}

#[cfg(opc_core_linked)]
impl Drop for MacCamSink {
    fn drop(&mut self) {
        // Safety: pairs with `open`.
        unsafe { opc_core_sys::opc_vcam_mac_close() };
    }
}
