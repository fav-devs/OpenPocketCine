//! Pixel repacking for the backends. BT.601 limited range, which is what a webcam
//! consumer assumes of a YUYV stream (V4L2 calls it SMPTE 170M).

/// RGBA8 to packed YUYV (4:2:2, two pixels in four bytes): `width × height × 2` bytes
/// for an even width. An odd width pads each row with a copy of its last pixel.
pub fn rgba_to_yuyv(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = Vec::with_capacity(w * h * 2);
    if rgba.len() < w * h * 4 {
        return out;
    }
    for y in 0..h {
        let row = &rgba[y * w * 4..(y + 1) * w * 4];
        let mut x = 0;
        while x < w {
            let left = pixel(row, x);
            let right = pixel(row, (x + 1).min(w - 1));
            let (y0, u0, v0) = yuv(left);
            let (y1, u1, v1) = yuv(right);
            let u = ((u16::from(u0) + u16::from(u1)) / 2) as u8;
            let v = ((u16::from(v0) + u16::from(v1)) / 2) as u8;
            out.extend_from_slice(&[y0, u, y1, v]);
            x += 2;
        }
    }
    out
}

fn pixel(row: &[u8], x: usize) -> (u8, u8, u8) {
    (row[x * 4], row[x * 4 + 1], row[x * 4 + 2])
}

/// BT.601 limited range, integer arithmetic as the ITU-R BT.601 tables give it.
fn yuv((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (clamp(y), clamp(u), clamp(v))
}

fn clamp(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_white_and_the_primaries_land_on_the_601_codes() {
        assert_eq!(yuv((0, 0, 0)), (16, 128, 128));
        assert_eq!(yuv((255, 255, 255)), (235, 128, 128));
        let (y, u, v) = yuv((255, 0, 0));
        assert!((y as i32 - 81).abs() <= 1 && (u as i32 - 90).abs() <= 1 && v == 240);
        let (y, u, v) = yuv((0, 0, 255));
        assert!((y as i32 - 41).abs() <= 1 && u == 240 && (v as i32 - 110).abs() <= 1);
    }

    #[test]
    fn a_row_packs_two_pixels_in_four_bytes_and_odd_widths_finish() {
        let rgba = [255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255];
        let packed = rgba_to_yuyv(3, 1, &rgba);
        assert_eq!(packed.len(), 8, "3 wide pads to 4 pixels × 2 bytes");
        assert_eq!(&packed[..4], &[235, 128, 16, 128]);
        assert_eq!(&packed[4..], &[235, 128, 235, 128]);
        assert!(
            rgba_to_yuyv(4, 1, &rgba).is_empty(),
            "a short buffer yields nothing"
        );
    }
}
