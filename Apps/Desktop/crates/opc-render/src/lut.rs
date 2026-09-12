//! `.cube` handling, delegated to the core.
//!
//! The renderer needs a 3D texture; it does not need an opinion about the file format.
//! Parsing, the supported size range, and Resolve's 65³ resample down to 64³ all stay in
//! `CubeLUT`, so a cube loads the same way on a PC as it does on the phones.

use std::ffi::{c_void, CString};

use opc_core_sys as sys;

use crate::error::RenderError;

/// A parsed colour cube, owned by the core.
#[derive(Debug)]
pub struct Lut {
    handle: *mut c_void,
}

impl Lut {
    /// Parses `.cube` text. The error message is the core's own.
    pub fn parse(text: &str) -> Result<Self, RenderError> {
        let mut message = [0u8; 256];
        // Safety: both slices outlive the call; the core writes at most 256 bytes.
        let handle = unsafe {
            sys::opc_lut_parse(
                text.as_ptr(),
                text.len(),
                message.as_mut_ptr(),
                message.len(),
            )
        };
        if handle.is_null() {
            let end = message
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(message.len());
            let detail = String::from_utf8_lossy(&message[..end]).into_owned();
            return Err(RenderError::Lut(if detail.is_empty() {
                "That .cube file could not be read.".to_string()
            } else {
                detail
            }));
        }
        Ok(Self { handle })
    }

    /// One of the core's built-in looks.
    pub fn built_in(name: &str, size: i32) -> Result<Self, RenderError> {
        let name = CString::new(name).map_err(|_| RenderError::Lut("Bad look name.".into()))?;
        // Safety: `name` outlives the call.
        let handle = unsafe { sys::opc_lut_builtin(name.as_ptr(), size) };
        if handle.is_null() {
            return Err(RenderError::Lut("That is not a built-in look.".to_string()));
        }
        Ok(Self { handle })
    }

    /// Lattice edge length. A texture upload wants `size³` RGBA texels.
    pub fn size(&self) -> u32 {
        // Safety: the handle is valid for the lifetime of `self`.
        unsafe { sys::opc_lut_size(self.handle) }.max(0) as u32
    }

    /// `size³ × 4` floats, red-fastest with alpha 1.
    pub fn rgba(&self) -> Vec<f32> {
        // Safety: probing with a null destination only reports the size needed.
        let needed = unsafe { sys::opc_lut_rgba(self.handle, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return Vec::new();
        }
        let mut out = vec![0.0_f32; needed as usize];
        // Safety: `out` has exactly the capacity the core just asked for.
        let written = unsafe { sys::opc_lut_rgba(self.handle, out.as_mut_ptr(), out.len()) };
        if written != needed {
            return Vec::new();
        }
        out
    }

    /// The core's own trilinear sample, for checking a GPU grade against.
    pub fn map(&self, red: f32, green: f32, blue: f32) -> (f32, f32, f32) {
        let mut out = [0.0_f32; 3];
        // Safety: `out` is three floats, which is what the core writes.
        let status = unsafe { sys::opc_lut_map(self.handle, red, green, blue, out.as_mut_ptr()) };
        if status != 1 {
            return (red, green, blue);
        }
        (out[0], out[1], out[2])
    }
}

impl Drop for Lut {
    fn drop(&mut self) {
        // Safety: retained by the core on creation and released exactly once.
        unsafe { sys::opc_lut_destroy(self.handle) }
    }
}

// The handle is plain heap state with no thread affinity and no interior mutation.
unsafe impl Send for Lut {}
unsafe impl Sync for Lut {}

/// The built-in look names, in the order the core lists them.
pub fn built_in_names() -> Vec<String> {
    // Safety: probing with a null destination only reports the size needed.
    let needed = unsafe { sys::opc_lut_builtin_names(std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let mut bytes = vec![0u8; needed as usize];
    // Safety: `bytes` has exactly the capacity the core just asked for.
    let written = unsafe { sys::opc_lut_builtin_names(bytes.as_mut_ptr(), bytes.len()) };
    if written != needed {
        return Vec::new();
    }
    String::from_utf8_lossy(&bytes)
        .split(',')
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}
