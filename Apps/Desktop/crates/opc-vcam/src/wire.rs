//! What crosses the process boundary to a camera the OS hosts elsewhere: on Windows
//! the Media Foundation source runs inside the Frame Server service and reads frames
//! from a named pipe the viewfinder serves.
//!
//! One frame is a 16-byte header — magic, width, height, payload length, little-endian —
//! followed by NV12 (BT.601 limited): a luma plane, then interleaved Cb/Cr at half
//! resolution. Every frame is complete on its own; a reader that joins late reads from
//! the next header.

/// `OPCV`.
pub const MAGIC: [u8; 4] = *b"OPCV";
pub const HEADER_LEN: usize = 16;
/// The pipe the viewfinder serves on Windows.
pub const PIPE_NAME: &str = r"\\.\pipe\OpenPocketCineVCam";
/// The media source's class id, as `opc_vcam_win` registers it and the viewfinder
/// names it to `MFCreateVirtualCamera`. Any change here changes both.
pub const SOURCE_CLSID: &str = "{6f0b4c1e-8d7a-4f3b-9c2e-5a1d7e9b3c40}";

/// The header of one frame on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub length: u32,
}

impl Header {
    /// The header for an NV12 frame of this size.
    pub fn nv12(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            length: nv12_len(width, height) as u32,
        }
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0; HEADER_LEN];
        out[..4].copy_from_slice(&MAGIC);
        out[4..8].copy_from_slice(&self.width.to_le_bytes());
        out[8..12].copy_from_slice(&self.height.to_le_bytes());
        out[12..16].copy_from_slice(&self.length.to_le_bytes());
        out
    }

    /// `None` for anything but a well-formed NV12 header.
    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Option<Self> {
        if bytes[..4] != MAGIC {
            return None;
        }
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let header = Self {
            width: word(4),
            height: word(8),
            length: word(12),
        };
        let sane = header.width > 0
            && header.height > 0
            && header.width <= 8192
            && header.height <= 8192
            && header.length as usize == nv12_len(header.width, header.height);
        sane.then_some(header)
    }
}

/// Bytes in an NV12 frame: the luma plane plus half again of chroma.
pub fn nv12_len(width: u32, height: u32) -> usize {
    let (w, h) = (width as usize, height as usize);
    w * h + 2 * w.div_ceil(2) * h.div_ceil(2)
}

/// RGBA8 to NV12, BT.601 limited range. Chroma is the mean of each 2×2 block; an odd
/// edge repeats its last row or column.
pub fn rgba_to_nv12(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return Vec::new();
    }
    let mut out = vec![0u8; nv12_len(width, height)];
    let (luma, chroma) = out.split_at_mut(w * h);
    let at = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        (
            i32::from(rgba[i]),
            i32::from(rgba[i + 1]),
            i32::from(rgba[i + 2]),
        )
    };
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = at(x, y);
            luma[y * w + x] = clamp(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16);
        }
    }
    let cw = w.div_ceil(2);
    for cy in 0..h.div_ceil(2) {
        for cx in 0..cw {
            let (mut u, mut v) = (0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let x = (cx * 2 + dx).min(w - 1);
                let y = (cy * 2 + dy).min(h - 1);
                let (r, g, b) = at(x, y);
                u += ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                v += ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
            }
            chroma[(cy * cw + cx) * 2] = clamp((u + 2) / 4);
            chroma[(cy * cw + cx) * 2 + 1] = clamp((v + 2) / 4);
        }
    }
    out
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_round_trips_and_refuses_nonsense() {
        let header = Header::nv12(1280, 720);
        assert_eq!(header.length, 1_382_400);
        let bytes = header.encode();
        assert_eq!(&bytes[..4], b"OPCV");
        assert_eq!(Header::decode(&bytes), Some(header));
        let mut wrong = bytes;
        wrong[12] ^= 1;
        assert_eq!(Header::decode(&wrong), None, "length must match the size");
        wrong = bytes;
        wrong[0] = b'X';
        assert_eq!(Header::decode(&wrong), None);
        assert_eq!(nv12_len(3, 3), 9 + 8);
    }

    #[test]
    fn nv12_carries_601_codes_with_averaged_chroma() {
        // 2×2: white, black / red, red.
        let rgba = [
            255, 255, 255, 255, 0, 0, 0, 255, //
            255, 0, 0, 255, 255, 0, 0, 255,
        ];
        let frame = rgba_to_nv12(2, 2, &rgba);
        assert_eq!(frame.len(), 6);
        assert_eq!(&frame[..4], &[235, 16, 82, 82]);
        // Chroma: (128 + 128 + 90 + 90) / 4 ≈ 109, (128 + 128 + 240 + 240) / 4 = 184.
        assert!((i32::from(frame[4]) - 109).abs() <= 1);
        assert_eq!(frame[5], 184);
        assert!(rgba_to_nv12(2, 2, &rgba[..8]).is_empty());
        // An odd size repeats the edge rather than reading past it.
        let odd = rgba_to_nv12(3, 1, &[0; 12]);
        assert_eq!(odd.len(), nv12_len(3, 1));
    }
}
