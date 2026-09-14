//! What the viewfinder does, driven with a fake clock.
//!
//! These are the tests that would have caught the things an operator notices: a record
//! key that does not roll, a countdown that never fires, a drag that points the camera
//! at the wrong half of the frame, a stick that keeps panning after the key is let go.

use opc_camera::{Command, Status};
use opc_monitor::shell::{Intent, Shell, TouchPhase};
use opc_ui::{Key, Phase};

fn sent(intents: &[Intent]) -> Vec<Command> {
    intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::Send(command) => Some(*command),
            _ => None,
        })
        .collect()
}

/// A 16:9 picture in a 16:9 window, so the fit is the whole window and the arithmetic
/// in a test stays readable.
fn framed() -> Shell {
    let mut shell = Shell::new();
    shell.set_window(1280, 720);
    shell.set_source(1920, 1080);
    shell
}

#[test]
fn space_rolls_and_r_stops() {
    let mut shell = framed();
    assert_eq!(sent(&shell.press(Key::Space, 0.0)), [Command::RecordStart]);
    assert_eq!(
        sent(&shell.press(Key::Char('r'), 0.0)),
        [Command::RecordStop]
    );
}

#[test]
fn the_timer_fires_once_and_then_leaves_the_camera_alone() {
    let mut shell = framed();
    assert!(
        sent(&shell.press(Key::Char('t'), 0.0)).is_empty(),
        "arming sends nothing"
    );
    assert!(shell.tick(1.0).is_empty(), "still counting");
    assert!(shell.tick(2.9).is_empty(), "still counting");
    assert_eq!(sent(&shell.tick(3.0)), [Command::RecordStart]);
    assert!(
        shell.tick(3.1).is_empty(),
        "a fired countdown must not roll again on every tick"
    );
}

#[test]
fn the_timer_can_be_cancelled_before_it_fires() {
    let mut shell = framed();
    shell.press(Key::Char('t'), 0.0);
    shell.press(Key::Char('t'), 1.0);
    assert!(
        shell.tick(10.0).is_empty(),
        "a cancelled countdown must never roll"
    );
}

#[test]
fn the_countdown_is_visible_while_it_runs() {
    let mut shell = framed();
    let quiet = shell.chrome(0.0).expect("chrome").pixels.clone();
    shell.press(Key::Char('t'), 0.0);
    let counting = shell.chrome(0.5).expect("chrome");
    assert_ne!(
        counting.pixels, quiet,
        "an armed countdown the operator cannot see is a take they will miss"
    );
}

#[test]
fn holding_an_arrow_pans_and_letting_go_stops() {
    let mut shell = framed();
    let down = sent(&shell.press(Key::Right, 0.0));
    assert_eq!(down.len(), 1);
    let resting = match sent(&shell.release(Key::Right, 0.1)).first() {
        Some(Command::GimbalStick { axis0, axis1 }) => (*axis0, *axis1),
        other => panic!("letting go must rest the stick, got {other:?}"),
    };
    assert_eq!(
        resting,
        (
            opc_ui::controls::STICK_CENTRE,
            opc_ui::controls::STICK_CENTRE
        ),
        "a gimbal that keeps panning after the key is up is a ruined shot"
    );
}

#[test]
fn a_held_stick_is_kept_alive_and_a_resting_one_is_not() {
    let mut shell = framed();
    shell.press(Key::Left, 0.0);
    assert!(shell.tick(0.1).is_empty(), "too soon to repeat");
    assert_eq!(sent(&shell.tick(0.25)).len(), 1, "the stick is kept alive");

    shell.release(Key::Left, 0.3);
    assert!(
        shell.tick(5.0).is_empty(),
        "a stick at rest must not be re-sent forever"
    );
}

#[test]
fn a_repeat_of_a_key_already_down_does_not_flood_the_camera() {
    let mut shell = framed();
    assert_eq!(sent(&shell.press(Key::Up, 0.0)).len(), 1);
    // Every keyboard auto-repeats. That must not become a command per repeat.
    for _ in 0..20 {
        assert!(shell.press(Key::Up, 0.0).is_empty());
    }
}

#[test]
fn a_drag_becomes_a_box_in_the_picture_the_operator_pointed_at() {
    let mut shell = framed();
    shell.pointer_down(320.0, 180.0);
    shell.pointer_moved(640.0, 360.0);
    let intents = shell.pointer_up(640.0, 360.0, 0.0);
    match sent(&intents).first() {
        Some(Command::TrackSet {
            x,
            y,
            width,
            height,
            ..
        }) => {
            assert!((x - 0.25).abs() < 0.01, "x was {x}");
            assert!((y - 0.25).abs() < 0.01, "y was {y}");
            assert!((width - 0.25).abs() < 0.01, "width was {width}");
            assert!((height - 0.25).abs() < 0.01, "height was {height}");
        }
        other => panic!("a drag should track, got {other:?}"),
    }
}

#[test]
fn a_drag_on_a_mirrored_picture_points_at_the_same_thing() {
    let mut shell = framed();
    shell.press(Key::Char('m'), 0.0);
    shell.pointer_down(320.0, 180.0);
    shell.pointer_moved(640.0, 360.0);
    match sent(&shell.pointer_up(640.0, 360.0, 0.0)).first() {
        Some(Command::TrackSet { x, width, .. }) => {
            // The operator dragged the left quarter of what they see; mirrored, that is
            // the right quarter of the sensor.
            assert!((x - 0.5).abs() < 0.01, "mirrored x was {x}");
            assert!((width - 0.25).abs() < 0.01);
        }
        other => panic!("a mirrored drag should still track, got {other:?}"),
    }
}

#[test]
fn a_click_does_not_clear_what_the_camera_is_already_following() {
    let mut shell = framed();
    shell.pointer_down(640.0, 360.0);
    assert!(
        shell.pointer_up(641.0, 361.0, 0.0).is_empty(),
        "a click is not a box, and must not send anything"
    );
}

#[test]
fn a_drag_that_starts_outside_the_picture_is_not_a_box() {
    let mut shell = Shell::new();
    shell.set_window(1280, 1280);
    shell.set_source(1920, 1080);
    // The window is square and the picture is 16:9, so the top of the window is a bar.
    shell.pointer_down(640.0, 10.0);
    shell.pointer_moved(900.0, 600.0);
    assert!(
        shell.pointer_up(900.0, 600.0, 0.0).is_empty(),
        "a drag begun on the letterbox bar points at nothing"
    );
}

#[test]
fn the_letterbox_is_where_the_renderer_put_it() {
    let mut shell = Shell::new();
    shell.set_window(1000, 1000);
    shell.set_source(1920, 1080);
    let fit = shell.fit();
    let (x, y, width, height) = opc_render::letterbox((1920, 1080), (1000, 1000));
    assert_eq!(
        (fit.x, fit.y, fit.width, fit.height),
        (
            f64::from(x),
            f64::from(y),
            f64::from(width),
            f64::from(height)
        ),
        "the shell and the blit must agree on where the picture is, to the pixel"
    );
}

#[test]
fn tracking_ids_never_repeat_and_never_reach_zero() {
    let mut shell = framed();
    let mut ids = Vec::new();
    for _ in 0..4 {
        shell.pointer_down(100.0, 100.0);
        shell.pointer_moved(500.0, 400.0);
        if let Some(Command::TrackSet { id, .. }) =
            sent(&shell.pointer_up(500.0, 400.0, 0.0)).first()
        {
            ids.push(*id);
        }
    }
    assert_eq!(ids.len(), 4);
    assert!(ids.iter().all(|id| *id != 0), "zero is not an id, {ids:?}");
    let mut sorted = ids.clone();
    sorted.dedup();
    assert_eq!(sorted.len(), 4, "ids must not repeat, {ids:?}");
}

#[test]
fn x_clears_tracking_and_takes_the_box_off_the_screen() {
    let mut shell = framed();
    shell.pointer_down(100.0, 100.0);
    shell.pointer_moved(500.0, 400.0);
    shell.pointer_up(500.0, 400.0, 0.0);
    let with_box = shell.chrome(0.1).expect("chrome").pixels.clone();

    assert_eq!(
        sent(&shell.press(Key::Char('x'), 0.2)),
        [Command::TrackClear]
    );
    let cleared = shell.chrome(0.2).expect("chrome");
    assert_ne!(cleared.pixels, with_box, "the box should be gone");
}

#[test]
fn a_committed_box_stops_being_drawn_rather_than_lying_about_the_subject() {
    let mut shell = framed();
    shell.pointer_down(100.0, 100.0);
    shell.pointer_moved(500.0, 400.0);
    shell.pointer_up(500.0, 400.0, 0.0);
    let confirmed = shell.chrome(0.1).expect("chrome").pixels.clone();
    shell.tick(2.0);
    let later = shell.chrome(2.0).expect("chrome");
    assert_ne!(
        later.pixels, confirmed,
        "the camera never says where the subject went, so the box must not stay"
    );
}

#[test]
fn the_format_keys_only_ask_for_what_the_body_says_it_has() {
    let mut shell = framed();
    shell.set_status(Status {
        available_formats: vec![(1, 24), (1, 60), (2, 24)],
        video_resolution: Some(1),
        video_frame_rate: Some(24),
        ..Status::default()
    });
    assert_eq!(
        sent(&shell.press(Key::Char(']'), 0.0)),
        [Command::SetVideoFormat {
            resolution: 1,
            frame_rate: 60
        }]
    );
    assert_eq!(
        sent(&shell.press(Key::Char('['), 0.0)),
        [Command::SetVideoFormat {
            resolution: 2,
            frame_rate: 24
        }]
    );
}

#[test]
fn a_camera_that_has_reported_nothing_yet_is_not_sent_a_guess() {
    let mut shell = framed();
    assert!(
        shell.press(Key::Char('['), 0.0).is_empty(),
        "no format list means no idea what the body shoots"
    );
    assert!(shell.press(Key::Char(']'), 0.0).is_empty());
}

#[test]
fn zoom_follows_the_body_rather_than_fighting_it() {
    let mut shell = framed();
    shell.set_status(Status {
        zoom_hundredths: Some(400),
        ..Status::default()
    });
    assert_eq!(
        sent(&shell.press(Key::Char('='), 0.0)),
        [Command::ZoomFactor(4.5)],
        "the next step should continue from where the lens actually is"
    );
}

#[test]
fn the_assist_keys_stay_out_of_the_camera() {
    let mut shell = framed();
    for key in ['z', 'p', 'l', 'm', 'h'] {
        assert!(
            sent(&shell.press(Key::Char(key), 0.0)).is_empty(),
            "{key} is the operator's own screen, not the camera's"
        );
    }
}

#[test]
fn the_cube_is_loaded_once_per_change_not_once_per_frame() {
    let mut shell = framed();
    assert_eq!(shell.take_lut_change(), None);
    shell.press(Key::Char('l'), 0.0);
    assert_eq!(shell.take_lut_change(), Some(true));
    assert_eq!(shell.take_lut_change(), None, "already applied");
    shell.press(Key::Char('l'), 0.0);
    assert_eq!(shell.take_lut_change(), Some(false));
}

#[test]
fn hiding_the_chrome_hides_all_of_it() {
    let mut shell = framed();
    shell.set_phase(Phase::Waiting);
    assert!(shell.chrome(0.0).is_some());
    shell.press(Key::Char('h'), 0.0);
    assert!(!shell.chrome_visible());
    assert!(
        shell.chrome(0.0).is_none(),
        "hidden chrome must not reach the screen at all"
    );
    shell.press(Key::Char('h'), 0.0);
    assert!(shell.chrome(0.0).is_some());
}

#[test]
fn the_chrome_is_the_size_of_the_window() {
    let mut shell = framed();
    let chrome = shell.chrome(0.0).expect("chrome");
    assert_eq!((chrome.width, chrome.height), (1280, 720));
    assert_eq!(chrome.pixels.len(), 1280 * 720 * 4);

    shell.set_window(800, 600);
    let chrome = shell.chrome(0.0).expect("chrome");
    assert_eq!((chrome.width, chrome.height), (800, 600));
}

#[test]
fn a_live_camera_with_nothing_wrong_leaves_the_middle_of_the_shot_clear() {
    let mut shell = framed();
    shell.set_phase(Phase::Live);
    let chrome = shell.chrome(0.0).expect("chrome");
    let middle = (chrome.height / 2) * chrome.width + chrome.width / 2;
    assert_eq!(
        chrome.pixels[middle as usize * 4 + 3],
        0,
        "the centre of the picture must not be covered"
    );
}

#[test]
fn the_rate_shown_counts_only_recent_frames() {
    let mut shell = framed();
    shell.set_phase(Phase::Live);
    for frame in 0..30 {
        shell.note_presented(f64::from(frame) / 30.0);
    }
    let busy = shell.chrome(1.0).expect("chrome").pixels.clone();
    // A long gap, and the rate must fall rather than remember a burst.
    shell.note_presented(60.0);
    let idle = shell.chrome(60.0).expect("chrome");
    assert_ne!(
        idle.pixels, busy,
        "a stalled feed must not still read 30 FPS"
    );
}

#[test]
fn escape_asks_to_close_and_sends_nothing() {
    let mut shell = framed();
    let intents = shell.press(Key::Escape, 0.0);
    assert_eq!(intents, [Intent::Quit]);
}

#[test]
fn a_window_with_no_size_yet_draws_no_chrome_instead_of_panicking() {
    let mut shell = Shell::new();
    assert!(shell.chrome(0.0).is_none());
    shell.pointer_down(10.0, 10.0);
    assert!(shell.pointer_up(20.0, 20.0, 0.0).is_empty());
    assert!(shell.tick(1.0).is_empty());
}

#[test]
fn a_cancelled_drag_is_abandoned_rather_than_sent() {
    let mut shell = framed();
    shell.pointer_down(320.0, 180.0);
    shell.pointer_moved(640.0, 360.0);
    shell.pointer_cancel();
    assert!(
        shell.pointer_up(640.0, 360.0, 0.0).is_empty(),
        "a palm on the screen must not point the camera at what it covered"
    );
}

#[test]
fn a_cancelled_drag_takes_its_box_off_the_screen() {
    let mut shell = framed();
    let clear = shell.chrome(0.0).expect("chrome").pixels.clone();
    shell.pointer_down(320.0, 180.0);
    shell.pointer_moved(640.0, 360.0);
    assert_ne!(
        shell.chrome(0.0).expect("chrome").pixels,
        clear,
        "the box should be drawn while it is being dragged"
    );
    shell.pointer_cancel();
    assert_eq!(
        shell.chrome(0.0).expect("chrome").pixels,
        clear,
        "and gone once the drag is taken away"
    );
}

#[test]
fn cancelling_when_nothing_is_being_dragged_does_nothing() {
    let mut shell = framed();
    shell.pointer_cancel();
    assert!(shell.pointer_up(100.0, 100.0, 0.0).is_empty());
}

/// A finger tracing the same box the mouse test drags: the middle quarter of the shot.
fn quarter_box(shell: &mut Shell, id: u64, now: f64) -> Vec<Intent> {
    shell.touch(id, TouchPhase::Started, 320.0, 180.0, now);
    shell.touch(id, TouchPhase::Moved, 640.0, 360.0, now);
    shell.touch(id, TouchPhase::Ended, 640.0, 360.0, now)
}

#[test]
fn a_finger_draws_the_same_box_a_mouse_does() {
    let mut shell = framed();
    match sent(&quarter_box(&mut shell, 7, 0.0)).first() {
        Some(Command::TrackSet {
            x,
            y,
            width,
            height,
            ..
        }) => {
            assert!((x - 0.25).abs() < 0.01, "x was {x}");
            assert!((y - 0.25).abs() < 0.01, "y was {y}");
            assert!((width - 0.25).abs() < 0.01, "width was {width}");
            assert!((height - 0.25).abs() < 0.01, "height was {height}");
        }
        other => panic!("a finger drag should track, got {other:?}"),
    }
}

#[test]
fn a_second_finger_cannot_take_over_a_box_being_drawn() {
    let mut shell = framed();
    shell.touch(1, TouchPhase::Started, 320.0, 180.0, 0.0);
    shell.touch(1, TouchPhase::Moved, 640.0, 360.0, 0.0);

    // A palm, or a second hand steadying the laptop.
    shell.touch(2, TouchPhase::Started, 100.0, 100.0, 0.0);
    shell.touch(2, TouchPhase::Moved, 110.0, 110.0, 0.0);
    assert!(
        shell
            .touch(2, TouchPhase::Ended, 110.0, 110.0, 0.0)
            .is_empty(),
        "the second finger must not send anything of its own"
    );

    // The first finger's box is still the one that lands, unchanged.
    match sent(&shell.touch(1, TouchPhase::Ended, 640.0, 360.0, 0.0)).first() {
        Some(Command::TrackSet { x, width, .. }) => {
            assert!((x - 0.25).abs() < 0.01, "x was {x}");
            assert!((width - 0.25).abs() < 0.01, "width was {width}");
        }
        other => panic!("the first finger should still track, got {other:?}"),
    }
}

#[test]
fn a_finger_that_lands_off_the_picture_does_not_lock_out_the_next_one() {
    let mut shell = Shell::new();
    shell.set_window(1280, 1280);
    shell.set_source(1920, 1080);
    // The window is square and the picture is 16:9, so the top is a letterbox bar.
    // Land below the top bar and above the picture: bar, not button.
    shell.touch(1, TouchPhase::Started, 640.0, 200.0, 0.0);

    // A finger that does land on the shot must still be able to draw.
    let fit = shell.fit();
    let inside = |u: f64, v: f64| (fit.x + u * fit.width, fit.y + v * fit.height);
    let (x0, y0) = inside(0.2, 0.2);
    let (x1, y1) = inside(0.6, 0.6);
    shell.touch(2, TouchPhase::Started, x0, y0, 0.0);
    shell.touch(2, TouchPhase::Moved, x1, y1, 0.0);
    assert!(
        !sent(&shell.touch(2, TouchPhase::Ended, x1, y1, 0.0)).is_empty(),
        "a finger on the bar must not claim the drag it never started"
    );
}

#[test]
fn a_cancelled_finger_abandons_its_box() {
    let mut shell = framed();
    shell.touch(3, TouchPhase::Started, 320.0, 180.0, 0.0);
    shell.touch(3, TouchPhase::Moved, 640.0, 360.0, 0.0);
    assert!(shell
        .touch(3, TouchPhase::Cancelled, 640.0, 360.0, 0.0)
        .is_empty());
    assert!(
        shell
            .touch(3, TouchPhase::Ended, 640.0, 360.0, 0.0)
            .is_empty(),
        "an Ended after a Cancelled must not resurrect the box"
    );
}

#[test]
fn a_finger_released_frees_the_screen_for_the_next_one() {
    let mut shell = framed();
    assert!(!sent(&quarter_box(&mut shell, 1, 0.0)).is_empty());
    assert!(
        !sent(&quarter_box(&mut shell, 2, 1.0)).is_empty(),
        "a new finger must be able to draw once the last one lifted"
    );
}

#[test]
fn a_tap_sends_nothing_by_finger_as_by_mouse() {
    let mut shell = framed();
    shell.touch(1, TouchPhase::Started, 640.0, 360.0, 0.0);
    assert!(
        shell
            .touch(1, TouchPhase::Ended, 641.0, 361.0, 0.0)
            .is_empty(),
        "a tap is not a box, and must not clear what the camera is following"
    );
}

#[test]
fn stray_phases_for_a_finger_nobody_is_tracking_do_nothing() {
    let mut shell = framed();
    // Events can arrive for a finger that started before the window had a size.
    shell.touch(9, TouchPhase::Moved, 100.0, 100.0, 0.0);
    assert!(shell
        .touch(9, TouchPhase::Ended, 200.0, 200.0, 0.0)
        .is_empty());
    assert!(shell
        .touch(9, TouchPhase::Cancelled, 200.0, 200.0, 0.0)
        .is_empty());
}

#[test]
fn gimbal_pad_throws_on_down_and_rests_on_release_and_cancel() {
    let mut shell = framed();
    shell.set_phase(opc_ui::Phase::Live);
    // The pad is 96 px square at the bottom left, centred on (252, 624) in this
    // 1280 × 720 layout. Below and right of centre throws both axes positive.
    let thrown = sent(&shell.control_down(280.0, 650.0, 0.0).expect("gimbal pad"));
    assert!(
        matches!(thrown.as_slice(), [Command::GimbalStick { axis0, axis1 }] if *axis0 > 1024 && *axis1 > 1024)
    );
    assert_eq!(
        sent(&shell.control_up(280.0, 650.0, 0.0)),
        [Command::GimbalStick {
            axis0: 1024,
            axis1: 1024
        }],
        "a release is an immediate rest, not a later timer tick"
    );

    shell.control_down(280.0, 650.0, 1.0).expect("gimbal pad");
    assert_eq!(
        sent(&shell.control_cancel()),
        [Command::GimbalStick {
            axis0: 1024,
            axis1: 1024
        }],
        "focus loss and touch cancellation must also rest the camera"
    );
}

#[test]
fn control_hit_rectangles_beat_tracking_and_buttons_keep_typed_commands() {
    let mut shell = framed();
    shell.set_phase(opc_ui::Phase::Live);
    // STILL is the third button of the trailing-bottom cluster at this layout. Its
    // target is 56 px, safely above the 44 px minimum, and it must not make a tracking
    // box.
    assert!(shell.is_control(1_184.0, 630.0));
    assert_eq!(
        sent(
            &shell
                .control_down(1_184.0, 630.0, 0.0)
                .expect("still button")
        ),
        [],
    );
    assert_eq!(
        sent(&shell.control_up(1_184.0, 630.0, 0.0)),
        [Command::ShootPhoto]
    );
    assert!(shell.pointer_up(1_184.0, 630.0, 0.0).is_empty());
}

#[test]
fn recovering_controls_are_inert_but_still_claim_their_area() {
    let mut shell = framed();
    shell.set_phase(opc_ui::Phase::Recovering);
    assert!(shell.is_control(950.0, 670.0));
    assert!(sent(&shell.control_down(950.0, 670.0, 0.0).expect("still button")).is_empty());
    assert!(shell.control_up(950.0, 670.0, 0.0).is_empty());
}

#[test]
fn every_primary_control_hit_target_survives_resize() {
    let mut shell = framed();
    // Centres of - / 1X / + / REC / STILL / FLIP / CTR / gimbal at 1280×720.
    for point in [
        (52.0, 668.0),
        (118.0, 668.0),
        (184.0, 668.0),
        (640.0, 668.0),
        (954.0, 668.0),
        (1_020.0, 668.0),
        (1_086.0, 668.0),
        (1_190.0, 554.0),
    ] {
        assert!(
            shell.is_control(point.0, point.1),
            "{point:?} should be a control"
        );
    }
    shell.set_window(1_920, 1_080);
    // The same trailing and bottom layout scales its plates, while remaining far above
    // the 44 px target floor.
    for point in [
        (60.0, 1_020.0),
        (142.0, 1_020.0),
        (224.0, 1_020.0),
        (960.0, 1_020.0),
        (1_506.0, 1_020.0),
        (1_588.0, 1_020.0),
        (1_670.0, 1_020.0),
        (1_812.0, 880.0),
    ] {
        assert!(
            shell.is_control(point.0, point.1),
            "{point:?} should resize with chrome"
        );
    }
}

// ── Sheets ───────────────────────────────────────────────────────────────────

#[test]
fn tab_opens_the_settings_and_escape_closes_them_before_it_quits() {
    let mut shell = framed();
    assert!(shell.press(Key::Tab, 0.0).is_empty());
    assert_eq!(
        shell.sheet(),
        Some(opc_monitor::sheets::SheetKind::Settings)
    );
    assert!(
        shell.press(Key::Escape, 0.0).is_empty(),
        "escape with a sheet open closes the sheet, not the window"
    );
    assert_eq!(shell.sheet(), None);
    assert_eq!(shell.press(Key::Escape, 0.0), [Intent::Quit]);
}

#[test]
fn e_opens_the_exposure_sheet_and_a_second_press_closes_it() {
    let mut shell = framed();
    shell.press(Key::Char('e'), 0.0);
    assert_eq!(
        shell.sheet(),
        Some(opc_monitor::sheets::SheetKind::Exposure)
    );
    shell.press(Key::Char('e'), 0.0);
    assert_eq!(shell.sheet(), None);
}

#[test]
fn an_open_sheet_owns_the_whole_window() {
    let mut shell = framed();
    shell.set_phase(opc_ui::Phase::Live);
    shell.press(Key::Tab, 0.0);
    // The window draws the frame before any tap can land on it.
    shell.chrome(0.0);
    // The middle of the picture would start a tracking box; under a sheet it cannot.
    assert!(shell.is_control(640.0, 500.0));
    shell.control_down(640.0, 500.0, 0.0).expect("scrim");
    shell.control_up(640.0, 500.0, 0.0);
    assert_eq!(shell.sheet(), None, "a tap on the scrim closes the sheet");
}

#[test]
fn a_chip_on_the_exposure_sheet_sends_the_typed_command() {
    let mut shell = framed();
    shell.set_phase(opc_ui::Phase::Live);
    shell.press(Key::Char('e'), 0.0);
    // The first chip of the first row ("Mode" → "Auto") at this 1280 × 720 layout:
    // the panel starts 16 px under the 56 px top bar, its header is 60 px, rows are
    // 56 px, and chips start 160 px in from the row's 16 px inset.
    shell.chrome(0.0);
    let (x, y) = (200.0 + 16.0 + 160.0 + 20.0, 56.0 + 16.0 + 60.0 + 8.0 + 28.0);
    assert!(shell.is_control(x, y));
    shell.control_down(x, y, 0.0).expect("chip");
    assert_eq!(
        sent(&shell.control_up(x, y, 0.0)),
        [Command::SetExpoMode(0x01)]
    );
    assert_eq!(
        shell.sheet(),
        Some(opc_monitor::sheets::SheetKind::Exposure),
        "picking a value keeps the sheet open for the next one"
    );
}
