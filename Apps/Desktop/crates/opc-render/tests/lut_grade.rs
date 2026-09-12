//! The grade, checked against the core's own sampler.
//!
//! Compiles only when the Swift core is linked (`just desktop-core`), because the cube
//! is parsed and sampled by `CubeLUT`. The point of the comparison is that the shader's
//! half-texel cube lookup lands on the same colour the core computes on the CPU — if the
//! two ever diverge, a PC watcher is showing a different picture from the phones.

#![cfg(opc_core_linked)]

use opc_decode::Picture;
use opc_render::{built_in_names, FeedRenderer, GradeOptions, Lut, Rgba};

const IDENTITY: &str = "LUT_3D_SIZE 2
0.0 0.0 0.0
1.0 0.0 0.0
0.0 1.0 0.0
1.0 1.0 0.0
0.0 0.0 1.0
1.0 0.0 1.0
0.0 1.0 1.0
1.0 1.0 1.0
";

/// Inverts every channel, so a grade is unmistakable.
const INVERT: &str = "LUT_3D_SIZE 2
1.0 1.0 1.0
0.0 1.0 1.0
1.0 0.0 1.0
0.0 0.0 1.0
1.0 1.0 0.0
0.0 1.0 0.0
1.0 0.0 0.0
0.0 0.0 0.0
";

struct Flat {
    luma: Vec<u8>,
    chroma_blue: Vec<u8>,
    chroma_red: Vec<u8>,
}

impl Flat {
    fn new(y: u8, cb: u8, cr: u8) -> Self {
        Self {
            luma: vec![y; 32 * 32],
            chroma_blue: vec![cb; 16 * 16],
            chroma_red: vec![cr; 16 * 16],
        }
    }

    fn picture(&self) -> Picture<'_> {
        Picture {
            width: 32,
            height: 32,
            is_keyframe: true,
            luma: &self.luma,
            chroma_blue: &self.chroma_blue,
            chroma_red: &self.chroma_red,
            luma_stride: 32,
            chroma_stride: 16,
        }
    }
}

fn renderer() -> Option<FeedRenderer> {
    FeedRenderer::new().ok()
}

fn centre(image: &Rgba) -> (u8, u8, u8) {
    let (r, g, b, _) = image.pixel(16, 16).expect("a centre pixel");
    (r, g, b)
}

fn render(renderer: &mut FeedRenderer, source: &Flat) -> (u8, u8, u8) {
    centre(
        &renderer
            .render(&source.picture(), (32, 32), GradeOptions::default())
            .expect("a picture should render"),
    )
}

#[test]
fn an_identity_cube_leaves_the_picture_where_it_was() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Flat::new(150, 140, 120);

    let plain = render(&mut renderer, &source);
    renderer
        .set_lut(Some(
            &Lut::parse(IDENTITY).expect("the identity cube should parse"),
        ))
        .expect("the cube should upload");
    let graded = render(&mut renderer, &source);

    for (before, after) in [
        (plain.0, graded.0),
        (plain.1, graded.1),
        (plain.2, graded.2),
    ] {
        let drift = (i32::from(before) - i32::from(after)).abs();
        assert!(
            drift <= 2,
            "identity should not move a channel, moved {drift}"
        );
    }
}

#[test]
fn clearing_the_cube_returns_the_ungraded_picture() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Flat::new(150, 128, 128);

    let plain = render(&mut renderer, &source);
    renderer
        .set_lut(Some(&Lut::parse(INVERT).unwrap()))
        .unwrap();
    let inverted = render(&mut renderer, &source);
    assert!(
        inverted.0.abs_diff(plain.0) > 40,
        "invert should visibly move the picture"
    );

    renderer.set_lut(None).expect("clearing should succeed");
    let restored = render(&mut renderer, &source);
    assert!(
        restored.0.abs_diff(plain.0) <= 2,
        "clearing should restore the picture"
    );
}

#[test]
fn the_gpu_grade_matches_the_core_sampler() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let look = Lut::built_in("Contrast", 17).expect("a built-in look");

    for (y, cb, cr) in [(90u8, 128u8, 128u8), (150, 110, 150), (200, 140, 120)] {
        let source = Flat::new(y, cb, cr);
        renderer.set_lut(None).unwrap();
        let plain = render(&mut renderer, &source);
        renderer.set_lut(Some(&look)).unwrap();
        let graded = render(&mut renderer, &source);

        let reference = look.map(
            f32::from(plain.0) / 255.0,
            f32::from(plain.1) / 255.0,
            f32::from(plain.2) / 255.0,
        );
        let expected = [
            (reference.0 * 255.0).round() as i32,
            (reference.1 * 255.0).round() as i32,
            (reference.2 * 255.0).round() as i32,
        ];
        let actual = [
            i32::from(graded.0),
            i32::from(graded.1),
            i32::from(graded.2),
        ];
        for channel in 0..3 {
            let drift = (expected[channel] - actual[channel]).abs();
            // A 17³ lattice sampled with 8-bit linear filtering will not match the
            // CPU's float trilinear exactly; a few codes is the honest tolerance.
            assert!(
                drift <= 6,
                "channel {channel} drifted {drift} (gpu {actual:?}, core {expected:?})"
            );
        }
    }
}

#[test]
fn a_split_grade_shows_the_cube_on_one_half_only() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Flat::new(150, 128, 128);
    renderer
        .set_lut(Some(&Lut::parse(INVERT).unwrap()))
        .unwrap();

    let rendered = renderer
        .render(
            &source.picture(),
            (32, 32),
            GradeOptions {
                split: true,
                split_vertical: true,
                ..GradeOptions::default()
            },
        )
        .expect("a split render should succeed");
    let left = rendered.pixel(4, 16).expect("a left pixel");
    let right = rendered.pixel(28, 16).expect("a right pixel");
    assert!(
        left.0.abs_diff(right.0) > 40,
        "a vertical split should grade one side only, {left:?} vs {right:?}"
    );
}

#[test]
fn every_built_in_look_loads_and_uploads() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let names = built_in_names();
    assert!(
        !names.is_empty(),
        "the core should publish its built-in looks"
    );
    for name in names {
        let look = Lut::built_in(&name, 17).unwrap_or_else(|_| panic!("{name} should load"));
        assert_eq!(look.size(), 17);
        renderer
            .set_lut(Some(&look))
            .unwrap_or_else(|_| panic!("{name} should upload"));
    }
}

#[test]
fn a_broken_cube_reports_the_cores_message() {
    let error = Lut::parse("LUT_3D_SIZE 2\n0.0 0.0 0.0\n").expect_err("a short cube is invalid");
    assert!(!error.to_string().is_empty());
}
