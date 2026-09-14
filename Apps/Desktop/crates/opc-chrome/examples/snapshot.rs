//! Render the chrome headlessly over a slate test frame and write PNGs.
//! `cargo run -p opc-chrome --example snapshot -- <out-dir>`
use std::time::Instant;

use opc_chrome::{Chrome, ChromeState, SheetRowState, SheetState};
use opc_ui::hud::Phase;

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let mut chrome = Chrome::new(Instant::now()).expect("chrome");

    // Name, phase, recording, window size. The last one is wider than 16:9 so the
    // side chrome parks in the gutters.
    let shots: Vec<(&str, Phase, bool, (u32, u32))> = vec![
        ("finding", Phase::Finding, false, (1280, 720)),
        ("live", Phase::Live, false, (1280, 720)),
        ("recording", Phase::Live, true, (1280, 720)),
        (
            "failed",
            Phase::Failed("camera went away".into()),
            false,
            (1280, 720),
        ),
        ("wide", Phase::Live, false, (1600, 720)),
        ("sheet", Phase::Live, false, (1280, 720)),
        ("settings", Phase::Live, false, (1280, 720)),
    ];

    for (name, phase, rec, (w, h)) in shots {
        // Fit a 16:9 picture into the window the way the shell does.
        let fit_w = ((h * 16) / 9).min(w);
        let fit_h = (fit_w * 9) / 16;
        let fit = ((w - fit_w) / 2, (h - fit_h) / 2, fit_w, fit_h);
        let failed = matches!(phase, Phase::Failed(_));
        let state = ChromeState {
            phase: &phase,
            shutter: "1/60".into(),
            iso: "400".into(),
            ev: "-0.3".into(),
            wb: "5600K".into(),
            link_state: if failed { "OFFLINE" } else { "LINK" },
            is_recording: rec,
            rec_elapsed: "00:12:34".into(),
            follow_on: true,
            format_label: "1080P·60".into(),
            expo_label: "AUTO".into(),
            battery_text: "15%".into(),
            battery_percent: 15,
            storage_text: "5:36:07".into(),
            zoom: 1.0,
            zoom_label: "1.0×".into(),
            mode: 3,
            photo_mode: false,
            controls_enabled: matches!(phase, Phase::Live),
            fit,
            countdown: (name == "live").then_some(3),
            fps_shown: 30,
            timecode: "01:02:03:04".into(),
            grid_on: name == "sheet",
            sheet: (name == "sheet" || name == "settings").then(|| {
                let row = |title: &str, options: &[&str], selected: Option<usize>, enabled| {
                    SheetRowState {
                        title: title.into(),
                        options: options.iter().map(|o| o.to_string()).collect(),
                        selected,
                        enabled,
                    }
                };
                if name == "settings" {
                    return SheetState {
                        title: "SETTINGS".into(),
                        tabs: vec!["CAMERA".into(), "AUDIO".into(), "ASSIST".into()],
                        tab: 0,
                        rows: vec![
                            row("Focus", &["Single", "Continuous"], Some(1), true),
                            row(
                                "White balance",
                                &["Auto", "2800K", "3200K", "4000K", "4500K", "5000K", "5600K"],
                                Some(6),
                                true,
                            ),
                            row("Color", &["Normal", "D-Log", "D-Log2"], Some(0), true),
                            row("Field of view", &["Wide", "Natural"], Some(0), true),
                            row(
                                "Gimbal mode",
                                &["Follow", "Tilt locked", "FPV"],
                                Some(0),
                                true,
                            ),
                            row("Gimbal speed", &["Slow", "Default", "Fast"], Some(1), true),
                        ],
                    };
                }
                SheetState {
                    title: "EXPOSURE".into(),
                    tabs: vec![],
                    tab: 0,
                    rows: vec![
                        row("Mode", &["Auto", "Manual"], Some(1), true),
                        row(
                            "ISO",
                            &["100", "200", "400", "800", "1600", "3200", "6400"],
                            Some(2),
                            true,
                        ),
                        row(
                            "ISO max",
                            &["100–800", "100–1600", "100–3200"],
                            Some(1),
                            false,
                        ),
                        row(
                            "Shutter",
                            &[
                                "1/8000", "1/4000", "1/2000", "1/1000", "1/500", "1/250", "1/200",
                                "1/120", "1/100", "1/60", "1/50", "1/30", "1/25",
                            ],
                            Some(9),
                            true,
                        ),
                        row(
                            "EV",
                            &["-1.0", "-0.7", "-0.3", "0.0", "+0.3", "+0.7", "+1.0"],
                            Some(2),
                            false,
                        ),
                    ],
                }
            }),
        };
        // Render twice so state changes settle (ReusedBuffer redraws dirty regions).
        let _ = chrome.render(&state, w, h);
        let canvas = chrome.render(&state, w, h);

        // Composite over a slate-gray test frame with a gradient so the overlay is legible;
        // the gutters outside the fitted picture are black like the shell's letterbox.
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let inside = x >= fit.0 && x < fit.0 + fit.2 && y >= fit.1 && y < fit.1 + fit.3;
                let g = if inside {
                    40 + ((x + y) * 60 / (w + h)) as u8
                } else {
                    0
                };
                let bg = [g, g + 4, g + 8, 255u8];
                let o = &canvas.pixels[i..i + 4];
                let a = o[3] as u32;
                for c in 0..3 {
                    rgba[i + c] = ((o[c] as u32 * a + bg[c] as u32 * (255 - a)) / 255) as u8;
                }
                rgba[i + 3] = 255;
            }
        }
        let path = out.join(format!("chrome-{name}.png"));
        write_png(&path, w, h, &rgba);
        println!("wrote {}", path.display());
    }
}
