//! The picture keeps its proportions.
//!
//! A stretched viewfinder is worse than a small one: framing is the thing it exists to
//! judge, and an operator cannot un-see a face the window made narrow.

use opc_decode::Picture;
use opc_render::{letterbox, FeedRenderer, GradeOptions};

struct Flat {
    luma: Vec<u8>,
    chroma: Vec<u8>,
    width: u32,
    height: u32,
}

impl Flat {
    fn new(width: u32, height: u32) -> Self {
        let chroma = (width.div_ceil(2) * height.div_ceil(2)) as usize;
        Self {
            // Bright, so anything black in the result is a bar and not the picture.
            luma: vec![235; (width * height) as usize],
            chroma: vec![128; chroma],
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
            chroma_blue: &self.chroma,
            chroma_red: &self.chroma,
            luma_stride: self.width as usize,
            chroma_stride: self.width.div_ceil(2) as usize,
        }
    }
}

#[test]
fn the_rectangle_is_centred_and_keeps_the_aspect() {
    // 16:9 into a square: full width, bars above and below.
    assert_eq!(letterbox((1920, 1080), (400, 400)), (0, 87, 400, 225));
    // 16:9 into a taller-than-wide window: same.
    assert_eq!(letterbox((1920, 1080), (400, 800)), (0, 287, 400, 225));
    // 4:3 into 16:9: full height, bars left and right.
    assert_eq!(letterbox((640, 480), (1600, 900)), (200, 0, 1200, 900));
    // Matching aspects fill exactly, with no bars to round into existence.
    assert_eq!(letterbox((1920, 1080), (1280, 720)), (0, 0, 1280, 720));
}

#[test]
fn a_degenerate_raster_asks_for_the_whole_display_rather_than_dividing_by_zero() {
    assert_eq!(letterbox((0, 1080), (640, 480)), (0, 0, 640, 480));
    assert_eq!(letterbox((1920, 0), (640, 480)), (0, 0, 640, 480));
    assert_eq!(letterbox((1920, 1080), (0, 0)), (0, 0, 0, 0));
}

#[test]
fn a_wide_picture_in_a_square_window_gets_bars_not_a_stretch() {
    let Ok(mut renderer) = FeedRenderer::new() else {
        eprintln!("skipping: no Vulkan device");
        return;
    };
    let source = Flat::new(64, 36);
    let rendered = renderer
        .render(&source.picture(), (128, 128), GradeOptions::default())
        .expect("a wide picture should render into a square");

    let (_, top, _, height) = letterbox((64, 36), (128, 128));
    assert!(top > 4, "a 16:9 picture in a square needs real bars");

    let bar = rendered.pixel(64, 1).expect("a pixel in the top bar");
    assert_eq!(bar, (0, 0, 0, 255), "the bar must be black, got {bar:?}");
    let bottom = rendered.pixel(64, 126).expect("a pixel in the bottom bar");
    assert_eq!(bottom, (0, 0, 0, 255), "the bottom bar too, got {bottom:?}");

    let inside = rendered
        .pixel(64, top + height / 2)
        .expect("a pixel in the picture");
    assert!(
        inside.0 > 240 && inside.1 > 240 && inside.2 > 240,
        "the picture itself should be white, got {inside:?}"
    );

    // The bars are exactly as tall as the arithmetic says, to the row.
    let first_lit = (0..128)
        .find(|row| rendered.pixel(64, *row).expect("a pixel").0 > 200)
        .expect("some row must be lit");
    assert_eq!(first_lit, top, "the picture must start where the fit says");
}

#[test]
fn a_matching_aspect_leaves_no_bars_at_all() {
    let Ok(mut renderer) = FeedRenderer::new() else {
        return;
    };
    let source = Flat::new(64, 36);
    let rendered = renderer
        .render(&source.picture(), (256, 144), GradeOptions::default())
        .expect("a 16:9 picture should fill a 16:9 display");
    assert!(
        rendered
            .pixels
            .chunks_exact(4)
            .all(|pixel| pixel[0] > 240 && pixel[3] == 255),
        "nothing should be letterboxed away when the shapes agree"
    );
}
