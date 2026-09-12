//! The swapchain path, driven without a display.
//!
//! `VK_EXT_headless_surface` gives a real `VkSurfaceKHR` and a real swapchain, so
//! acquire, submit, present, and recreate are the same code a window drives. That is
//! worth testing precisely because a window is the part this environment cannot open.

use opc_decode::Picture;
use opc_render::{FeedRenderer, GradeOptions, Presented, RenderError};

struct Flat {
    luma: Vec<u8>,
    chroma: Vec<u8>,
    width: u32,
    height: u32,
}

impl Flat {
    fn new(width: u32, height: u32, y: u8) -> Self {
        let chroma = (width.div_ceil(2) * height.div_ceil(2)) as usize;
        Self {
            luma: vec![y; (width * height) as usize],
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

/// Skips where there is no Vulkan driver, or no headless-surface support on it.
fn windowed(width: u32, height: u32) -> Option<FeedRenderer> {
    match FeedRenderer::headless_window(width, height) {
        Ok(renderer) => {
            eprintln!("presenting on: {}", renderer.device_name());
            Some(renderer)
        }
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

/// Presents until something is actually shown, so a rebuild does not fail a test.
fn present_until_shown(
    renderer: &mut FeedRenderer,
    picture: &Picture<'_>,
    attempts: usize,
) -> bool {
    for _ in 0..attempts {
        match renderer.present(picture, GradeOptions::default()) {
            Ok(Presented::Shown) => return true,
            Ok(Presented::Rebuilt) => continue,
            Err(error) => panic!("present failed: {error}"),
        }
    }
    false
}

#[test]
fn a_swapchain_shows_a_frame() {
    let Some(mut renderer) = windowed(320, 240) else {
        return;
    };
    let source = Flat::new(160, 120, 180);
    assert!(present_until_shown(&mut renderer, &source.picture(), 4));
}

#[test]
fn frames_keep_presenting_in_a_row() {
    let Some(mut renderer) = windowed(256, 144) else {
        return;
    };
    let source = Flat::new(128, 72, 150);
    let picture = source.picture();
    assert!(present_until_shown(&mut renderer, &picture, 4));

    // More than the frames in flight, so the fences and per-image semaphores get reused.
    let mut shown = 0;
    for _ in 0..12 {
        match renderer.present(&picture, GradeOptions::default()) {
            Ok(Presented::Shown) => shown += 1,
            Ok(Presented::Rebuilt) => {}
            Err(error) => panic!("present failed: {error}"),
        }
    }
    assert!(shown >= 10, "only {shown} of 12 frames reached the screen");
}

#[test]
fn a_resize_rebuilds_the_swapchain() {
    let Some(mut renderer) = windowed(320, 240) else {
        return;
    };
    let source = Flat::new(160, 120, 180);
    let picture = source.picture();
    assert!(present_until_shown(&mut renderer, &picture, 4));

    renderer.resize(200, 150);
    // The frame that notices a resize rebuilds instead of showing.
    assert_eq!(
        renderer.present(&picture, GradeOptions::default()).unwrap(),
        Presented::Rebuilt
    );
    assert_eq!(renderer.surface_size(), Some((200, 150)));
    assert!(present_until_shown(&mut renderer, &picture, 4));
}

#[test]
fn repeated_resizes_settle() {
    let Some(mut renderer) = windowed(320, 240) else {
        return;
    };
    let source = Flat::new(160, 120, 180);
    let picture = source.picture();
    for (width, height) in [(400u32, 300u32), (128, 96), (640, 360), (320, 240)] {
        renderer.resize(width, height);
        assert!(
            present_until_shown(&mut renderer, &picture, 4),
            "nothing shown after resizing to {width}x{height}"
        );
        assert_eq!(renderer.surface_size(), Some((width, height)));
    }
}

#[test]
fn a_minimised_window_draws_nothing_instead_of_failing() {
    let Some(mut renderer) = windowed(320, 240) else {
        return;
    };
    let source = Flat::new(160, 120, 180);
    let picture = source.picture();
    renderer.resize(0, 0);
    assert_eq!(
        renderer.present(&picture, GradeOptions::default()).unwrap(),
        Presented::Rebuilt
    );
    // And it comes back.
    renderer.resize(320, 240);
    assert!(present_until_shown(&mut renderer, &picture, 4));
}

#[test]
fn a_still_can_be_grabbed_while_presenting() {
    let Some(mut renderer) = windowed(320, 240) else {
        return;
    };
    let source = Flat::new(160, 120, 200);
    let picture = source.picture();
    assert!(present_until_shown(&mut renderer, &picture, 4));

    let grabbed = renderer
        .render(&picture, (160, 120), GradeOptions::default())
        .expect("a still should render while a swapchain exists");
    assert_eq!((grabbed.width, grabbed.height), (160, 120));
}

#[test]
fn an_offscreen_renderer_says_it_has_no_window() {
    let Ok(mut renderer) = FeedRenderer::new() else {
        return;
    };
    let source = Flat::new(32, 32, 128);
    assert_eq!(renderer.surface_size(), None);
    assert!(matches!(
        renderer.present(&source.picture(), GradeOptions::default()),
        Err(RenderError::NotPresenting)
    ));
}
