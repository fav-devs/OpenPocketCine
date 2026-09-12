//! The chrome pass.
//!
//! The HUD is the one thing an operator reads rather than looks at, so these check the
//! two ways it can silently go wrong: chrome that never reaches the screen, and chrome
//! that tints the picture under it.

use opc_decode::Picture;
use opc_render::{FeedRenderer, GradeOptions, Rgba};

fn renderer() -> Option<FeedRenderer> {
    match FeedRenderer::new() {
        Ok(renderer) => Some(renderer),
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

/// A flat mid-grey picture, so anything painted over it is obvious.
struct Grey {
    luma: Vec<u8>,
    chroma: Vec<u8>,
    width: u32,
    height: u32,
}

impl Grey {
    fn new(width: u32, height: u32) -> Self {
        let chroma = (width.div_ceil(2) * height.div_ceil(2)) as usize;
        Self {
            luma: vec![126; (width * height) as usize],
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

/// Chrome covering the left half only, so the right half proves the picture survived.
fn left_half(width: u32, height: u32, colour: [u8; 4]) -> Rgba {
    let mut pixels = vec![0_u8; (width * height * 4) as usize];
    for row in 0..height {
        for column in 0..width / 2 {
            let at = ((row * width + column) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&colour);
        }
    }
    Rgba {
        width,
        height,
        pixels,
    }
}

#[test]
fn chrome_paints_over_the_picture_and_leaves_the_rest_alone() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Grey::new(64, 64);
    let plain = renderer
        .render(&source.picture(), (64, 64), GradeOptions::default())
        .expect("a picture with no chrome should render");
    let under = plain.pixel(16, 32).expect("a left-hand pixel");

    renderer.set_overlay(&left_half(64, 64, [255, 0, 0, 255]));
    let painted = renderer
        .render(&source.picture(), (64, 64), GradeOptions::default())
        .expect("a picture with chrome should render");

    let (r, g, b, a) = painted.pixel(16, 32).expect("a painted pixel");
    assert!(
        r > 240 && g < 16 && b < 16,
        "opaque chrome should replace the picture, got {r},{g},{b}"
    );
    assert_eq!(a, 255, "the composite is always opaque");
    assert_eq!(
        painted.pixel(48, 32),
        plain.pixel(48, 32),
        "where the chrome is transparent the picture must be untouched"
    );
    assert_ne!(under, (r, g, b, a), "the picture was grey before the chrome");
}

#[test]
fn clearing_the_chrome_brings_the_picture_back() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Grey::new(48, 48);
    renderer.set_overlay(&left_half(48, 48, [255, 255, 255, 255]));
    let painted = renderer
        .render(&source.picture(), (48, 48), GradeOptions::default())
        .expect("a picture with chrome should render");
    assert!(painted.pixel(8, 24).expect("a painted pixel").0 > 240);

    renderer.clear_overlay();
    let cleared = renderer
        .render(&source.picture(), (48, 48), GradeOptions::default())
        .expect("a picture with no chrome should render");
    assert_eq!(
        cleared.pixel(8, 24),
        cleared.pixel(40, 24),
        "with the chrome gone both halves are the same flat picture"
    );
}

#[test]
fn half_alpha_chrome_blends_rather_than_replaces() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Grey::new(32, 32);
    let plain = renderer
        .render(&source.picture(), (32, 32), GradeOptions::default())
        .expect("a picture should render");
    let (base, _, _, _) = plain.pixel(8, 16).expect("a left-hand pixel");

    renderer.set_overlay(&left_half(32, 32, [255, 0, 0, 128]));
    let painted = renderer
        .render(&source.picture(), (32, 32), GradeOptions::default())
        .expect("a picture should render");
    let (r, g, _, _) = painted.pixel(8, 16).expect("a blended pixel");
    assert!(
        r > base && g < base,
        "half-alpha red should pull red up and green down from {base}, got {r},{g}"
    );
    assert!(r < 250, "half alpha should not read as opaque, got {r}");
}

#[test]
fn opacity_fades_the_chrome_without_dimming_the_picture() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Grey::new(32, 32);
    renderer.set_overlay(&left_half(32, 32, [255, 255, 255, 255]));
    renderer.set_overlay_opacity(0.0);
    let faded = renderer
        .render(&source.picture(), (32, 32), GradeOptions::default())
        .expect("a picture should render");
    assert_eq!(
        faded.pixel(8, 16),
        faded.pixel(24, 16),
        "chrome at zero opacity must leave the picture exactly as it was"
    );

    renderer.set_overlay_opacity(1.0);
    let solid = renderer
        .render(&source.picture(), (32, 32), GradeOptions::default())
        .expect("a picture should render");
    assert!(solid.pixel(8, 16).expect("a painted pixel").0 > 240);
}

#[test]
fn chrome_from_a_stale_raster_is_cropped_rather_than_refused() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    // The shell drew at 64x64; the window is already 32x32. One frame is wrong, not
    // an error the shell has to unwind.
    renderer.set_overlay(&left_half(64, 64, [255, 0, 0, 255]));
    let source = Grey::new(32, 32);
    let painted = renderer
        .render(&source.picture(), (32, 32), GradeOptions::default())
        .expect("mismatched chrome should still render");
    assert!(painted.pixel(4, 16).expect("a painted pixel").0 > 240);
}

#[test]
fn chrome_survives_a_raster_change() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    renderer.set_overlay(&left_half(32, 32, [255, 0, 0, 255]));
    for (width, height) in [(32_u32, 32_u32), (64, 64), (32, 32)] {
        renderer.set_overlay(&left_half(width, height, [255, 0, 0, 255]));
        let source = Grey::new(width, height);
        let painted = renderer
            .render(&source.picture(), (width, height), GradeOptions::default())
            .expect("each raster should render");
        let (r, _, _, _) = painted.pixel(2, height / 2).expect("a painted pixel");
        assert!(r > 240, "chrome should follow a resize, got {r} at {width}");
    }
}
