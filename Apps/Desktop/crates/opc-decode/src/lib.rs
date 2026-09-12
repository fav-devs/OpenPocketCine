//! HEVC and AVC decoding for the desktop watcher.
//!
//! The relay delivers whole Annex-B access units, so this is a plain feed-and-drain
//! decoder with no parser in front of it. Reordering is disabled: the camera's stream
//! has no B-frames and the host's re-encode keeps it that way, so any delay a decoder
//! adds here is delay an operator sees.

use std::ffi::c_void;
use std::fmt;

pub mod annexb;

#[allow(non_camel_case_types)]
mod sys {
    use std::ffi::c_void;

    pub const OPC_DECODE_FRAME: i32 = 1;
    pub const OPC_DECODE_AGAIN: i32 = 2;
    pub const OPC_DECODE_FORMAT_YUV420P: i32 = 0;

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct OpcDecodedFrame {
        pub width: i32,
        pub height: i32,
        pub format: i32,
        pub is_keyframe: i32,
        pub plane: [*const u8; 3],
        pub stride: [i32; 3],
    }

    impl Default for OpcDecodedFrame {
        fn default() -> Self {
            Self {
                width: 0,
                height: 0,
                format: 0,
                is_keyframe: 0,
                plane: [std::ptr::null(); 3],
                stride: [0; 3],
            }
        }
    }

    extern "C" {
        pub fn opc_decoder_create(codec: i32) -> *mut c_void;
        pub fn opc_decoder_destroy(decoder: *mut c_void);
        pub fn opc_decoder_send(decoder: *mut c_void, data: *const u8, length: usize) -> i32;
        pub fn opc_decoder_receive(decoder: *mut c_void, out: *mut OpcDecodedFrame) -> i32;
        pub fn opc_decoder_flush(decoder: *mut c_void);
    }
}

/// Which camera family's stream this is. Pocket sends HEVC; Nano sends AVC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Hevc,
    H264,
}

impl Codec {
    fn tag(self) -> i32 {
        match self {
            Self::Hevc => 0,
            Self::H264 => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// FFmpeg has no decoder for this codec, or the context would not open.
    Unavailable(Codec),
    /// The access unit was refused.
    Send,
    /// The decoder failed while producing a picture.
    Receive,
    /// A picture arrived in a pixel format this shell does not draw.
    UnsupportedFormat,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(codec) => write!(f, "no {codec:?} decoder is available"),
            Self::Send => write!(f, "the decoder refused an access unit"),
            Self::Receive => write!(f, "the decoder failed while producing a picture"),
            Self::UnsupportedFormat => {
                write!(
                    f,
                    "the decoder produced a pixel format this build cannot draw"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// One decoded 8-bit 4:2:0 picture, borrowed from the decoder.
///
/// The borrow ends at the next call that touches the decoder, which matches the C
/// contract exactly: the planes point into FFmpeg's frame and are reused.
#[derive(Debug, Clone, Copy)]
pub struct Picture<'a> {
    pub width: u32,
    pub height: u32,
    pub is_keyframe: bool,
    pub luma: &'a [u8],
    pub chroma_blue: &'a [u8],
    pub chroma_red: &'a [u8],
    pub luma_stride: usize,
    pub chroma_stride: usize,
}

impl Picture<'_> {
    /// Chroma planes are half resolution in both directions, rounded up.
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }
}

/// A decoded picture copied out of the decoder.
///
/// The decoder reuses its frame memory, so anything that outlives the next `receive`
/// needs its own copy. Rows are packed tight here, unlike the decoder's padded strides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPicture {
    pub width: u32,
    pub height: u32,
    pub is_keyframe: bool,
    luma: Vec<u8>,
    chroma_blue: Vec<u8>,
    chroma_red: Vec<u8>,
}

impl OwnedPicture {
    /// Packs a borrowed picture, dropping the decoder's row padding.
    pub fn copy_from(picture: &Picture<'_>) -> Self {
        let (chroma_width, chroma_height) = picture.chroma_size();
        let pack = |plane: &[u8], stride: usize, width: u32, height: u32| {
            let mut out = Vec::with_capacity((width as usize) * (height as usize));
            for row in 0..height as usize {
                let start = row * stride;
                out.extend_from_slice(&plane[start..start + width as usize]);
            }
            out
        };
        Self {
            width: picture.width,
            height: picture.height,
            is_keyframe: picture.is_keyframe,
            luma: pack(
                picture.luma,
                picture.luma_stride,
                picture.width,
                picture.height,
            ),
            chroma_blue: pack(
                picture.chroma_blue,
                picture.chroma_stride,
                chroma_width,
                chroma_height,
            ),
            chroma_red: pack(
                picture.chroma_red,
                picture.chroma_stride,
                chroma_width,
                chroma_height,
            ),
        }
    }

    /// Borrows it back as a `Picture` the renderer can take.
    pub fn picture(&self) -> Picture<'_> {
        Picture {
            width: self.width,
            height: self.height,
            is_keyframe: self.is_keyframe,
            luma: &self.luma,
            chroma_blue: &self.chroma_blue,
            chroma_red: &self.chroma_red,
            luma_stride: self.width as usize,
            chroma_stride: self.width.div_ceil(2) as usize,
        }
    }
}

/// A live decoder. Feed access units, drain pictures.
#[derive(Debug)]
pub struct Decoder {
    handle: *mut c_void,
    codec: Codec,
}

impl Decoder {
    pub fn new(codec: Codec) -> Result<Self, DecodeError> {
        // Safety: the shim returns null rather than a partly built decoder.
        let handle = unsafe { sys::opc_decoder_create(codec.tag()) };
        if handle.is_null() {
            return Err(DecodeError::Unavailable(codec));
        }
        Ok(Self { handle, codec })
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// Feeds one whole Annex-B access unit.
    pub fn send(&mut self, access_unit: &[u8]) -> Result<(), DecodeError> {
        if access_unit.is_empty() {
            return Ok(());
        }
        // Safety: the shim borrows the slice only for the duration of this call.
        let status =
            unsafe { sys::opc_decoder_send(self.handle, access_unit.as_ptr(), access_unit.len()) };
        if status < 0 {
            return Err(DecodeError::Send);
        }
        Ok(())
    }

    /// Pulls the next picture, or `None` when the decoder needs more input.
    pub fn receive(&mut self) -> Result<Option<Picture<'_>>, DecodeError> {
        let mut raw = sys::OpcDecodedFrame::default();
        // Safety: `raw` is a live record and the handle is valid for `self`.
        let status = unsafe { sys::opc_decoder_receive(self.handle, &mut raw) };
        match status {
            sys::OPC_DECODE_AGAIN => return Ok(None),
            sys::OPC_DECODE_FRAME => {}
            status if status < 0 => return Err(DecodeError::Receive),
            _ => return Ok(None),
        }
        if raw.format != sys::OPC_DECODE_FORMAT_YUV420P {
            return Err(DecodeError::UnsupportedFormat);
        }
        if raw.width <= 0 || raw.height <= 0 || raw.plane.iter().any(|plane| plane.is_null()) {
            return Err(DecodeError::Receive);
        }

        let height = raw.height as usize;
        let chroma_height = height.div_ceil(2);
        let luma_stride = raw.stride[0].max(0) as usize;
        let chroma_stride = raw.stride[1].max(0) as usize;
        if raw.stride[1] != raw.stride[2] {
            return Err(DecodeError::UnsupportedFormat);
        }

        // Safety: FFmpeg guarantees `stride * height` readable bytes per plane, and the
        // returned lifetime is tied to `&mut self` so nothing can outlive the next call.
        let (luma, chroma_blue, chroma_red) = unsafe {
            (
                std::slice::from_raw_parts(raw.plane[0], luma_stride * height),
                std::slice::from_raw_parts(raw.plane[1], chroma_stride * chroma_height),
                std::slice::from_raw_parts(raw.plane[2], chroma_stride * chroma_height),
            )
        };

        Ok(Some(Picture {
            width: raw.width as u32,
            height: raw.height as u32,
            is_keyframe: raw.is_keyframe != 0,
            luma,
            chroma_blue,
            chroma_red,
            luma_stride,
            chroma_stride,
        }))
    }

    /// Drops decoder state after a reconnect, the way the phone shells flush for recovery.
    pub fn flush(&mut self) {
        // Safety: the handle is valid for the lifetime of `self`.
        unsafe { sys::opc_decoder_flush(self.handle) }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // Safety: created by `opc_decoder_create` and destroyed exactly once.
        unsafe { sys::opc_decoder_destroy(self.handle) }
    }
}

// FFmpeg decoder state has no thread affinity and `&mut self` gates every call, so the
// decoder may move between threads but is never shared without a lock.
unsafe impl Send for Decoder {}
