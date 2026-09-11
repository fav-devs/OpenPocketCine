//! Zebra and peaking, checked on the pixels they paint.
//!
//! Both are already in the Android shell's `feed.frag`; this covers the desktop shell
//! driving them. Peaking adds the two extra passes (`peaking_blur`, `peaking_mask`)
//! whose output that shader samples.

use opc_decode::Picture;
use opc_render::{FeedRenderer, GradeOptions, Peaking, PeakingSense, Rgba, Zebra};

/// A picture built from a per-pixel luma function, with neutral chroma.
struct Source {
    luma: Vec<u8>,
    chroma: Vec<u8>,
    width: u32,
    height: u32,
}

impl Source {
    fn flat(width: u32, height: u32, y: u8) -> Self {
        Self::from_fn(width, height, |_, _| y)
    }

    /// A hard vertical edge down the middle: the thing peaking is meant to find.
    fn vertical_edge(width: u32, height: u32) -> Self {
        Self::from_fn(width, height, |x, _| if x < width / 2 { 30 } else { 220 })
    }

    fn from_fn<F: Fn(u32, u32) -> u8>(width: u32, height: u32, sample: F) -> Self {
        let mut luma = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                luma.push(sample(x, y));
            }
        }
        Self {
            luma,
            chroma: vec![128; (width.div_ceil(2) * height.div_ceil(2)) as usize],
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

fn renderer() -> Option<FeedRenderer> {
    FeedRenderer::new().ok()
}

fn draw(renderer: &mut FeedRenderer, source: &Source, options: GradeOptions) -> Rgba {
    renderer
        .render(&source.picture(), (source.width, source.height), options)
        .expect("a picture should render")
}

/// Pixels where red clearly dominates — the colour both assists paint with here.
fn is_reddish(r: u8, g: u8, b: u8) -> bool {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    r > 140 && r - g > 60 && r - b > 60
}

fn reddish(image: &Rgba) -> usize {
    image
        .pixels
        .chunks_exact(4)
        .filter(|pixel| is_reddish(pixel[0], pixel[1], pixel[2]))
        .count()
}

#[test]
fn zebra_highlights_stripe_the_bright_areas() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Source::flat(64, 64, 235);

    let plain = draw(&mut renderer, &source, GradeOptions::default());
    assert_eq!(
        reddish(&plain),
        0,
        "nothing should be painted with zebra off"
    );

    let zebra = Zebra {
        highlight_color: [1.0, 0.0, 0.0, 1.0],
        ..Zebra::highlight(0.5)
    };
    let striped = draw(
        &mut renderer,
        &source,
        GradeOptions {
            zebra: Some(zebra),
            ..GradeOptions::default()
        },
    );
    let painted = reddish(&striped);
    let total = (64 * 64) as usize;
    // Diagonal stripes, so roughly half the frame — not all of it, which is the check
    // that these are stripes rather than a flood fill.
    assert!(
        painted > total / 8 && painted < total * 7 / 8,
        "{painted} of {total} pixels striped"
    );
}

#[test]
fn zebra_leaves_a_dark_picture_alone() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Source::flat(64, 64, 40);
    let zebra = Zebra {
        highlight_color: [1.0, 0.0, 0.0, 1.0],
        ..Zebra::highlight(0.9)
    };
    let striped = draw(
        &mut renderer,
        &source,
        GradeOptions {
            zebra: Some(zebra),
            ..GradeOptions::default()
        },
    );
    assert_eq!(
        reddish(&striped),
        0,
        "nothing near the threshold should be painted"
    );
}

#[test]
fn zebra_midtones_stripe_a_band_and_not_outside_it() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let midtone = Zebra {
        midtone_color: [1.0, 0.0, 0.0, 1.0],
        ..Zebra::midtone(0.5, 0.06)
    };
    let options = GradeOptions {
        zebra: Some(midtone),
        ..GradeOptions::default()
    };

    // Limited-range 126 lands near half scale, inside the band.
    let inside = draw(&mut renderer, &Source::flat(64, 64, 126), options);
    assert!(reddish(&inside) > 0, "a midtone should be striped");

    // Well above the band.
    let outside = draw(&mut renderer, &Source::flat(64, 64, 225), options);
    assert_eq!(
        reddish(&outside),
        0,
        "a highlight should not take midtone stripes"
    );
}

#[test]
fn an_empty_zebra_paints_nothing() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let source = Source::flat(64, 64, 235);
    let painted = draw(
        &mut renderer,
        &source,
        GradeOptions {
            zebra: Some(Zebra::default()),
            ..GradeOptions::default()
        },
    );
    assert_eq!(reddish(&painted), 0);
}

#[test]
fn peaking_strokes_an_edge_and_leaves_flat_areas_alone() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let edged = Source::vertical_edge(96, 96);

    let plain = draw(&mut renderer, &edged, GradeOptions::default());
    assert_eq!(
        reddish(&plain),
        0,
        "nothing should be stroked with peaking off"
    );

    let options = GradeOptions {
        peaking: Some(Peaking {
            sense: PeakingSense::High,
            color: [1.0, 0.0, 0.0, 1.0],
        }),
        ..GradeOptions::default()
    };
    let stroked = draw(&mut renderer, &edged, options);
    let painted = reddish(&stroked);
    assert!(painted > 0, "a hard edge should be stroked");
    // A stroke, not a wash: the edge is one column of a 96-wide frame.
    assert!(
        painted < (96 * 96) / 4,
        "{painted} pixels is a fill, not a stroke"
    );

    let flat = draw(&mut renderer, &Source::flat(96, 96, 150), options);
    assert_eq!(reddish(&flat), 0, "a flat picture has no edges to stroke");
}

#[test]
fn peaking_finds_the_edge_where_it_actually_is() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let edged = Source::vertical_edge(96, 96);
    let options = GradeOptions {
        peaking: Some(Peaking {
            sense: PeakingSense::High,
            color: [1.0, 0.0, 0.0, 1.0],
        }),
        ..GradeOptions::default()
    };
    let stroked = draw(&mut renderer, &edged, options);

    let column_of = |x: u32| {
        (0..96)
            .filter(|y| {
                let (r, g, b, _) = stroked.pixel(x, *y).expect("a pixel");
                is_reddish(r, g, b)
            })
            .count()
    };
    let at_edge: usize = (46..50).map(column_of).sum();
    let far_away: usize = (10..14).map(column_of).sum();
    assert!(
        at_edge > far_away,
        "the stroke should sit on the edge: {at_edge} at the middle, {far_away} away"
    );
}

#[test]
fn turning_peaking_on_and_off_between_frames_is_safe() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let edged = Source::vertical_edge(64, 64);
    let on = GradeOptions {
        peaking: Some(Peaking {
            sense: PeakingSense::High,
            color: [1.0, 0.0, 0.0, 1.0],
        }),
        ..GradeOptions::default()
    };
    // Rebinds the mask descriptor each way; this is the path that would trip a
    // descriptor written while a frame was still in flight.
    for step in 0..6 {
        let options = if step % 2 == 0 {
            on
        } else {
            GradeOptions::default()
        };
        let drawn = draw(&mut renderer, &edged, options);
        if step % 2 == 1 {
            assert_eq!(reddish(&drawn), 0);
        }
    }
}

#[test]
fn the_sensitivity_presets_match_the_android_ladder() {
    assert_eq!(PeakingSense::default(), PeakingSense::Medium);
    assert!(PeakingSense::Low.ratio_threshold() > PeakingSense::High.ratio_threshold());
    assert!(PeakingSense::Low.noise_gate() > PeakingSense::High.noise_gate());
    assert_eq!(PeakingSense::Medium.ratio_threshold(), 2.10);
    assert_eq!(PeakingSense::Medium.noise_gate(), 0.001_74);
}
