//! Runs the feed pipeline on whatever Vulkan device this machine has.
//!
//! A software device (llvmpipe / lavapipe) is a perfectly good target: the point is that
//! the passes are wired correctly and the shaders produce the right pixels, not how fast
//! they do it.

use opc_decode::Picture;
use opc_render::{FeedRenderer, GradeOptions, Rgba};

/// A flat picture at one YCbCr triple, so the conversion can be checked by hand.
struct Flat {
    luma: Vec<u8>,
    chroma_blue: Vec<u8>,
    chroma_red: Vec<u8>,
    width: u32,
    height: u32,
}

impl Flat {
    fn new(width: u32, height: u32, y: u8, cb: u8, cr: u8) -> Self {
        let chroma = (width.div_ceil(2) * height.div_ceil(2)) as usize;
        Self {
            luma: vec![y; (width * height) as usize],
            chroma_blue: vec![cb; chroma],
            chroma_red: vec![cr; chroma],
            width,
            height,
        }
    }

    fn picture(&self) -> Picture<'_> {
        Picture {
            width: self.width,
            height: self.height,
            is_keyframe: true,
            luma: &self.luma,
            chroma_blue: &self.chroma_blue,
            chroma_red: &self.chroma_red,
            luma_stride: self.width as usize,
            chroma_stride: self.width.div_ceil(2) as usize,
        }
    }
}

/// Skips rather than fails where there is no Vulkan driver at all — a machine without
/// one is a missing environment, not a broken pipeline.
fn renderer() -> Option<FeedRenderer> {
    match FeedRenderer::new() {
        Ok(renderer) => {
            eprintln!("feed pipeline on: {}", renderer.device_name());
            Some(renderer)
        }
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

fn centre(image: &Rgba) -> (u8, u8, u8, u8) {
    image
        .pixel(image.width / 2, image.height / 2)
        .expect("a centre pixel")
}

#[test]
fn limited_range_black_and_white_land_where_they_should() {
    let Some(mut renderer) = renderer() else {
        return;
    };

    let black = Flat::new(64, 64, 16, 128, 128);
    let rendered = renderer
        .render(&black.picture(), (64, 64), GradeOptions::default())
        .expect("black should render");
    let (r, g, b, a) = centre(&rendered);
    assert!(
        r <= 2 && g <= 2 && b <= 2,
        "limited-range 16 should be black, got {r},{g},{b}"
    );
    assert_eq!(a, 255);

    let white = Flat::new(64, 64, 235, 128, 128);
    let rendered = renderer
        .render(&white.picture(), (64, 64), GradeOptions::default())
        .expect("white should render");
    let (r, g, b, _) = centre(&rendered);
    assert!(
        r >= 252 && g >= 252 && b >= 252,
        "limited-range 235 should be white, got {r},{g},{b}"
    );
}

#[test]
fn chroma_moves_the_hue_the_way_bt709_says() {
    let Some(mut renderer) = renderer() else {
        return;
    };

    // Cr above neutral pushes red; Cb above neutral pushes blue.
    let reddish = Flat::new(32, 32, 126, 128, 200);
    let rendered = renderer
        .render(&reddish.picture(), (32, 32), GradeOptions::default())
        .expect("a picture should render");
    let (r, g, b, _) = centre(&rendered);
    assert!(r > g && r > b, "Cr should push red, got {r},{g},{b}");

    let bluish = Flat::new(32, 32, 126, 200, 128);
    let rendered = renderer
        .render(&bluish.picture(), (32, 32), GradeOptions::default())
        .expect("a picture should render");
    let (r, _, b, _) = centre(&rendered);
    assert!(b > r, "Cb should push blue, got {r},_,{b}");
}

#[test]
fn the_display_raster_is_what_comes_back() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Flat::new(64, 48, 126, 128, 128);
    let rendered = renderer
        .render(&source.picture(), (256, 192), GradeOptions::default())
        .expect("a stretched picture should render");
    assert_eq!(rendered.width, 256);
    assert_eq!(rendered.height, 192);
    assert_eq!(rendered.pixels.len(), 256 * 192 * 4);
}

#[test]
fn changing_raster_between_frames_rebuilds_cleanly() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    for (width, height) in [(32u32, 32u32), (64, 48), (32, 32), (128, 72)] {
        let source = Flat::new(width, height, 180, 128, 128);
        let rendered = renderer
            .render(&source.picture(), (width, height), GradeOptions::default())
            .expect("each raster should render");
        assert_eq!((rendered.width, rendered.height), (width, height));
    }
}

#[test]
fn a_raster_the_pipeline_cannot_take_is_refused() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Flat::new(32, 32, 126, 128, 128);
    assert!(renderer
        .render(&source.picture(), (0, 32), GradeOptions::default())
        .is_err());
}

mod decoded {
    use super::{renderer, GradeOptions, Rgba};
    use opc_decode::{annexb, Codec, Decoder};

    const STREAM: &[u8] = include_bytes!("../../opc-decode/tests/fixtures/testsrc.h265");

    fn first_picture_rendered(options: GradeOptions, display: (u32, u32)) -> Option<Rgba> {
        let mut renderer = renderer()?;
        let mut decoder = Decoder::new(Codec::Hevc).expect("an HEVC decoder");
        for unit in annexb::access_units(STREAM) {
            decoder
                .send(unit)
                .expect("the decoder should accept an access unit");
            if let Some(picture) = decoder.receive().expect("decoding should not fail") {
                return Some(
                    renderer
                        .render(&picture, display, options)
                        .expect("a decoded picture should render"),
                );
            }
        }
        None
    }

    #[test]
    fn a_decoded_picture_reaches_the_screen() {
        let Some(rendered) = first_picture_rendered(GradeOptions::default(), (320, 240)) else {
            return;
        };
        assert_eq!((rendered.width, rendered.height), (320, 240));
        let lit = rendered
            .pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[0] > 8 || pixel[1] > 8 || pixel[2] > 8)
            .count();
        assert!(
            lit > 1000,
            "a test pattern should not render black, {lit} lit pixels"
        );
        assert!(rendered.pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
    }

    #[test]
    fn mirror_reverses_the_picture_horizontally() {
        let Some(plain) = first_picture_rendered(GradeOptions::default(), (64, 64)) else {
            return;
        };
        let mirrored = first_picture_rendered(
            GradeOptions {
                mirror: true,
                ..GradeOptions::default()
            },
            (64, 64),
        )
        .expect("the mirrored render should also succeed");

        let row = 32;
        let left = plain.pixel(4, row).expect("a left pixel");
        let reflected = mirrored.pixel(64 - 1 - 4, row).expect("the matching pixel");
        let difference = (i32::from(left.0) - i32::from(reflected.0)).abs()
            + (i32::from(left.1) - i32::from(reflected.1)).abs()
            + (i32::from(left.2) - i32::from(reflected.2)).abs();
        assert!(
            difference < 24,
            "mirroring should reflect the picture, off by {difference}"
        );
    }
}

mod stills {
    use super::{renderer, Flat, GradeOptions};
    use opc_decode::{annexb, Codec, Decoder};

    const STREAM: &[u8] = include_bytes!("../../opc-decode/tests/fixtures/testsrc.h265");

    #[test]
    fn a_render_encodes_as_a_readable_png() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let source = Flat::new(48, 48, 180, 128, 128);
        let image = renderer
            .render(&source.picture(), (48, 48), GradeOptions::default())
            .expect("a picture should render");

        let mut encoded = Vec::new();
        opc_render::encode(&mut encoded, &image).expect("the render should encode");
        assert!(encoded.starts_with(&[0x89, b'P', b'N', b'G']), "not a PNG");

        let decoder = png::Decoder::new(encoded.as_slice());
        let mut reader = decoder.read_info().expect("the PNG should parse");
        let mut out = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut out).expect("the PNG should decode");
        assert_eq!((info.width, info.height), (48, 48));
        assert_eq!(&out[..info.buffer_size()], image.pixels.as_slice());
    }

    #[test]
    fn a_decoded_frame_survives_the_whole_chain() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let mut decoder = Decoder::new(Codec::Hevc).expect("an HEVC decoder");
        let mut written = 0;
        for unit in annexb::access_units(STREAM).into_iter().take(3) {
            decoder
                .send(unit)
                .expect("the decoder should accept an access unit");
            while let Some(picture) = decoder.receive().expect("decoding should not fail") {
                let image = renderer
                    .render(&picture, (160, 120), GradeOptions::default())
                    .expect("a decoded picture should render");
                let mut encoded = Vec::new();
                opc_render::encode(&mut encoded, &image).expect("it should encode");
                assert!(
                    encoded.len() > 128,
                    "a PNG of a test pattern should not be empty"
                );
                written += 1;
            }
        }
        assert!(
            written > 0,
            "at least one frame should have gone all the way through"
        );
    }
}
