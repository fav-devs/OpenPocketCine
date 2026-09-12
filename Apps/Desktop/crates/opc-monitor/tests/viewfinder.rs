//! The seam between the shell and the screen.
//!
//! The shell's tests prove it decides the right things and the renderer's prove it draws
//! the right pixels. Neither catches chrome that is decided correctly and then never
//! reaches the glass — which is what an operator would actually see. This drives both
//! together on a software Vulkan device, with no window and no camera.

use opc_camera::Status;
use opc_decode::Picture;
use opc_monitor::Shell;
use opc_render::{FeedRenderer, Rgba};
use opc_ui::{Key, Phase};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

struct Grey {
    luma: Vec<u8>,
    chroma: Vec<u8>,
}

impl Grey {
    fn new() -> Self {
        Self {
            luma: vec![126; (WIDTH * HEIGHT) as usize],
            chroma: vec![128; (WIDTH.div_ceil(2) * HEIGHT.div_ceil(2)) as usize],
        }
    }

    fn picture(&self) -> Picture<'_> {
        Picture {
            width: WIDTH,
            height: HEIGHT,
            is_keyframe: true,
            luma: &self.luma,
            chroma_blue: &self.chroma,
            chroma_red: &self.chroma,
            luma_stride: WIDTH as usize,
            chroma_stride: WIDTH.div_ceil(2) as usize,
        }
    }
}

fn renderer() -> Option<FeedRenderer> {
    match FeedRenderer::new() {
        Ok(renderer) => Some(renderer),
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

/// One frame, exactly as the window draws it: the shell's chrome over the picture.
fn frame(renderer: &mut FeedRenderer, shell: &mut Shell, now: f64) -> Rgba {
    match shell.chrome(now) {
        Some(chrome) => renderer.set_overlay(chrome),
        None => renderer.clear_overlay(),
    }
    let source = Grey::new();
    renderer
        .render(&source.picture(), (WIDTH, HEIGHT), shell.grade_options())
        .expect("a frame should render")
}

fn white_pixels(image: &Rgba) -> usize {
    image
        .pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200)
        .count()
}

fn shell_at(phase: Phase) -> Shell {
    let mut shell = Shell::new();
    shell.set_window(WIDTH, HEIGHT);
    shell.set_source(WIDTH, HEIGHT);
    shell.set_phase(phase);
    shell
}

#[test]
fn what_the_camera_is_set_to_reaches_the_glass() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut shell = shell_at(Phase::Live);
    let bare = frame(&mut renderer, &mut shell, 0.0);
    assert_eq!(
        white_pixels(&bare),
        0,
        "a live camera with nothing to say should draw no chrome over mid-grey"
    );

    shell.set_status(Status {
        iso: Some(400),
        shutter_denominator: Some(50),
        battery_percent: Some(72),
        ..Status::default()
    });
    let lit = frame(&mut renderer, &mut shell, 0.0);
    assert!(
        white_pixels(&lit) > 50,
        "the readout must be legible on screen, {} white pixels",
        white_pixels(&lit)
    );
}

#[test]
fn hiding_the_chrome_takes_it_off_the_screen_and_not_just_out_of_the_model() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut shell = shell_at(Phase::Waiting);
    let showing = frame(&mut renderer, &mut shell, 0.0);
    assert!(white_pixels(&showing) > 50, "the wait message should show");

    shell.press(Key::Char('h'), 0.0);
    let hidden = frame(&mut renderer, &mut shell, 0.0);
    assert_eq!(
        white_pixels(&hidden),
        0,
        "hidden chrome that still composites is chrome that is not hidden"
    );

    shell.press(Key::Char('h'), 0.0);
    let back = frame(&mut renderer, &mut shell, 0.0);
    assert!(white_pixels(&back) > 50, "and it must come back");
}

#[test]
fn the_record_lamp_is_red_on_screen_while_the_body_is_rolling() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut shell = shell_at(Phase::Live);
    shell.set_status(Status {
        is_recording: true,
        record_elapsed: 12,
        ..Status::default()
    });
    let rolling = frame(&mut renderer, &mut shell, 0.0);
    let red = rolling
        .pixels
        .chunks_exact(4)
        .filter(|pixel| {
            i32::from(pixel[0]) - i32::from(pixel[1]) > 80
                && i32::from(pixel[0]) - i32::from(pixel[2]) > 80
        })
        .count();
    assert!(
        red > 20,
        "a rolling camera must be obvious, {red} red pixels"
    );
}

#[test]
fn a_tracking_drag_is_drawn_where_it_was_dragged() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut shell = shell_at(Phase::Live);
    shell.pointer_down(64.0, 36.0);
    shell.pointer_moved(192.0, 108.0);
    let dragging = frame(&mut renderer, &mut shell, 0.0);

    // The box is drawn in the tracking colour, which is green enough to find.
    let green = |x: u32, y: u32| {
        dragging
            .pixel(x, y)
            .is_some_and(|(r, g, b, _)| g > 180 && g > r + 60 && g > b.saturating_add(20))
    };
    assert!(
        (60..70).any(|x| green(x, 36)),
        "the top-left corner of the box should be on screen"
    );
    assert!(
        !green(WIDTH / 2, HEIGHT - 4),
        "and nothing should be drawn well outside it"
    );
}

#[test]
fn the_chrome_follows_the_window_when_it_changes_size() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut shell = shell_at(Phase::Waiting);
    frame(&mut renderer, &mut shell, 0.0);

    // A resize between frames must not leave the renderer holding chrome of the old
    // size, which would smear the readout across the new one.
    shell.set_window(WIDTH * 2, HEIGHT * 2);
    let chrome = shell.chrome(0.0).expect("chrome");
    assert_eq!((chrome.width, chrome.height), (WIDTH * 2, HEIGHT * 2));
}
