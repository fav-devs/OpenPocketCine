//! Thumbnail JPEGs to RGBA for the chrome.

use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// A decoded picture, tightly packed RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub fn decode_jpeg(bytes: &[u8]) -> Result<Rgba, String> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    let pixels = decoder.decode().map_err(|error| error.to_string())?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| "no dimensions".to_string())?;
    Ok(Rgba {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

/// Halves a picture until it is no wider than `max_width`, averaging 2×2 blocks.
/// Thumbnails are small already; this only tames a full-size still.
pub fn fit(picture: Rgba, max_width: u32) -> Rgba {
    let mut current = picture;
    while current.width > max_width && current.width >= 2 && current.height >= 2 {
        let (w, h) = (current.width / 2, current.height / 2);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                for c in 0..4 {
                    let at = |dx: u32, dy: u32| {
                        current.pixels
                            [(((y * 2 + dy) * current.width + x * 2 + dx) * 4 + c) as usize]
                            as u32
                    };
                    pixels[((y * w + x) * 4 + c) as usize] =
                        ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4) as u8;
                }
            }
        }
        current = Rgba {
            width: w,
            height: h,
            pixels,
        };
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_halves_until_narrow_enough() {
        let picture = Rgba {
            width: 8,
            height: 4,
            pixels: vec![200; 8 * 4 * 4],
        };
        let small = fit(picture, 3);
        assert_eq!((small.width, small.height), (2, 1));
        assert!(small.pixels.iter().all(|p| *p == 200));
    }
}
