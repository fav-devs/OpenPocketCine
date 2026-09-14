//! Render the chrome headlessly over a slate test frame and write PNGs.
//! `cargo run -p opc-chrome --example snapshot -- <out-dir>`
use std::time::Instant;

use opc_chrome::{
    AssistChip, CellState, Chrome, ChromeState, LegendBand, LibraryState, Overlays, PlateKind,
    PlateState, PlayerState, Screen, SelectionState, SheetRowState, SheetState,
};
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
    // A synthetic thumbnail: a warm gradient with a dark bar, standing in for a .scr.
    let (tw, th) = (320u32, 180u32);
    let mut thumb = vec![0u8; (tw * th * 4) as usize];
    for y in 0..th {
        for x in 0..tw {
            let i = ((y * tw + x) * 4) as usize;
            let dark = y > th * 2 / 3;
            thumb[i] = if dark { 30 } else { 120 + (x * 100 / tw) as u8 };
            thumb[i + 1] = if dark { 30 } else { 90 + (y * 60 / th) as u8 };
            thumb[i + 2] = if dark { 34 } else { 70 };
            thumb[i + 3] = 255;
        }
    }
    // A synthetic waveform plate: a dark panel with a bright band across it.
    let (pw, ph) = (250u32, 153u32);
    let mut plate = vec![0u8; (pw * ph * 4) as usize];
    for y in 0..ph {
        for x in 0..pw {
            let i = ((y * pw + x) * 4) as usize;
            let band = (60..70).contains(&y) || (y > 100 && y < 104 && x % 2 == 0);
            plate[i] = if band { 200 } else { 6 };
            plate[i + 1] = if band { 230 } else { 9 };
            plate[i + 2] = if band { 210 } else { 8 };
            plate[i + 3] = if band { 255 } else { 184 };
        }
    }
    chrome.set_plate("wave", pw, ph, &plate);
    for n in 0..5 {
        chrome.set_thumb(
            &format!("DCIM/DJI_001/DJI_2026081412525{n}_003{n}_D.MP4"),
            tw,
            th,
            &thumb,
        );
    }
    let frames: Vec<(u32, u32, Vec<u8>)> = (0..8)
        .map(|i| {
            let mut frame = thumb.clone();
            for px in frame.chunks_mut(4) {
                px[0] = px[0].saturating_sub(i * 12);
            }
            (tw, th, frame)
        })
        .collect();
    chrome.set_strip("DCIM/DJI_001/DJI_20260814125252_0032_D.MP4", &frames);

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
        ("output", Phase::Live, false, (1280, 720)),
        ("assists", Phase::Live, false, (1280, 720)),
        ("library", Phase::Live, false, (1280, 720)),
        ("player", Phase::Live, false, (1280, 720)),
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
            zoom_max: 12.0,
            zoom_stops: vec![1.0, 3.0, 6.0, 12.0],
            notice: if name == "failed" {
                "NO ANSWER FROM THE CAMERA".into()
            } else {
                String::new()
            },
            mode: 3,
            photo_mode: false,
            controls_enabled: matches!(phase, Phase::Live),
            fit,
            countdown: (name == "live").then_some(3),
            fps_shown: 30,
            timecode: "01:02:03:04".into(),
            overlays: if name == "assists" {
                let feed = (fit.0 as f32, fit.1 as f32, fit.2 as f32, fit.3 as f32);
                let frame = |ratio: f32| {
                    let (fx, fy, fw, fh) = feed;
                    let (w, h) = if fw / fh > ratio {
                        (fh * ratio, fh)
                    } else {
                        (fw, fw / ratio)
                    };
                    (fx + (fw - w) / 2.0, fy + (fh - h) / 2.0, w, h)
                };
                Overlays {
                    grid_thirds: true,
                    grid_phi: false,
                    grid_diagonal: true,
                    guides: vec![frame(2.39), frame(1.0)],
                    guide_mask: true,
                    crosshair: true,
                    legend: [
                        ("0–4", [0.35, 0.0, 0.5]),
                        ("5", [0.2, 0.2, 0.9]),
                        ("10–12", [0.3, 0.5, 1.0]),
                        ("41–48", [0.2, 0.8, 0.3]),
                        ("61–70", [0.95, 0.5, 0.75]),
                        ("92–93", [1.0, 0.85, 0.2]),
                        ("94–95", [1.0, 0.6, 0.1]),
                        ("96–98", [1.0, 0.3, 0.1]),
                        ("99–100", [1.0, 0.0, 0.0]),
                    ]
                    .into_iter()
                    .map(|(label, rgb)| LegendBand {
                        label: label.into(),
                        rgb,
                    })
                    .collect(),
                }
            } else {
                Overlays {
                    grid_thirds: name == "sheet",
                    ..Overlays::default()
                }
            },
            parts: opc_chrome::ChromeParts::default(),
            plates: if name == "assists" {
                vec![
                    PlateState {
                        tool_index: 4,
                        name: "wave".into(),
                        title: "WAVE".into(),
                        x: fit.0 as f32 + 12.0,
                        y: (fit.1 + fit.3) as f32 - 152.0 - 153.0 - 12.0,
                        width: 250.0,
                        height: 153.0,
                        kind: PlateKind::Image,
                    },
                    PlateState {
                        tool_index: 9,
                        name: "nd".into(),
                        title: "ND".into(),
                        x: fit.0 as f32 + 12.0,
                        y: 148.0,
                        width: 84.0,
                        height: 30.0,
                        kind: PlateKind::Text("ND32".into()),
                    },
                    PlateState {
                        tool_index: 14,
                        name: "audio".into(),
                        title: "AUDIO".into(),
                        x: fit.0 as f32 + 12.0,
                        y: 190.0,
                        width: 28.0,
                        height: 168.0,
                        kind: PlateKind::Audio {
                            left: 0.72,
                            right: 0.55,
                            left_peak: 0.9,
                            right_peak: 0.7,
                        },
                    },
                ]
            } else {
                Vec::new()
            },
            assist_bar: (name == "assists").then(|| {
                [
                    ("LUT", true, true, 0),
                    ("PEAK", false, true, 0),
                    ("FALSE", true, true, 0),
                    ("ZEBRA", false, true, 1),
                    ("WAVE", true, true, 1),
                    ("PARADE", false, true, 1),
                    ("HISTO", false, true, 2),
                    ("VECTOR", false, true, 2),
                    ("LIGHTS", false, true, 2),
                    ("ND", false, true, 2),
                    ("GUIDES", true, true, 3),
                    ("GRID", true, true, 3),
                    ("CROSS", true, true, 3),
                    ("MIRROR", false, true, 4),
                    ("AUDIO", false, true, 5),
                ]
                .into_iter()
                .map(|(label, on, available, group)| AssistChip {
                    label: label.into(),
                    on,
                    available,
                    group,
                })
                .collect()
            }),
            move_text: if name == "recording" {
                "MOVE · A→B 3.2 / 8.0 s".into()
            } else {
                String::new()
            },
            screen: match name {
                "library" => Screen::Library,
                "player" => Screen::Player,
                _ => Screen::Viewfinder,
            },
            library: (name == "library").then(|| {
                // Three days of takes, laid out the way the shell does at 1280 px.
                let (cell_w, cell_h, gap): (f32, f32, f32) = (220.0, 124.0, 10.0);
                let columns = ((1280.0 - 48.0 + gap) / (cell_w + gap)).floor() as usize;
                let mut cells = Vec::new();
                let mut y = 0.0;
                for (day, count) in [("Today", 1usize), ("2026-09-04", 3), ("2026-08-23", 7)] {
                    cells.push(CellState {
                        path: String::new(),
                        header: true,
                        title: day.into(),
                        meta: String::new(),
                        x: 0.0,
                        y,
                        is_video: false,
                        starred: false,
                        cached: false,
                        selected: false,
                        checked: false,
                        burst: 0,
                    });
                    y += 44.0 + gap;
                    for n in 0..count {
                        let index = cells.len();
                        cells.push(CellState {
                            path: format!(
                                "DCIM/DJI_001/DJI_2026081412525{}_003{}_D.MP4",
                                index % 5,
                                index % 5
                            ),
                            header: false,
                            title: String::new(),
                            meta: if n == 2 {
                                "JPG".into()
                            } else {
                                format!("0:{:02}", 12 + index * 7 % 50)
                            },
                            x: (n % columns) as f32 * (cell_w + gap),
                            y: y + (n / columns) as f32 * (cell_h + gap),
                            is_video: n != 2,
                            starred: index == 3,
                            cached: index == 2,
                            selected: index == 2,
                            checked: index == 2 || index == 5,
                            burst: if index == 4 { 5 } else { 0 },
                        });
                    }
                    y += count.div_ceil(columns) as f32 * (cell_h + gap);
                }
                LibraryState {
                    tab: 0,
                    local: false,
                    sort_label: "Newest".into(),
                    status: "11 files · 4.2 GB free".into(),
                    cells,
                    content_height: y,
                    cell_width: cell_w,
                    cell_height: cell_h,
                    selection: Some(SelectionState {
                        title: "DJI_20260814125252_0032_D.MP4".into(),
                        meta: "0:26 · 3840x2160 · 30 fps · 412.5 MB".into(),
                        is_video: true,
                        starred: false,
                        cached: true,
                        deletable: true,
                        delete_armed: false,
                        progress: Some(0.62),
                        note: "Proxy on disk".into(),
                        burst: 0,
                        expanded: false,
                    }),
                    selecting: true,
                    checked_count: 2,
                    batch_armed: false,
                }
            }),
            player: (name == "player").then(|| PlayerState {
                path: "DCIM/DJI_001/DJI_20260814125252_0032_D.MP4".into(),
                title: "DJI_20260814125252_0032_D.MP4".into(),
                tag: "Low-Res".into(),
                info: "0:26 · 3840x2160 · 30 fps · 412.5 MB".into(),
                show_info: true,
                position_label: "00:09".into(),
                duration_label: "00:26".into(),
                progress: 0.35,
                playing: true,
                starred: true,
                cached: false,
                deletable: true,
                delete_armed: false,
                lut_on: true,
                zebra_on: false,
                peaking_on: false,
                is_photo: false,
                conform_label: "120 → 24".into(),
                conform_on: true,
                conform_available: true,
            }),
            sheet: (name == "sheet" || name == "settings" || name == "output").then(|| {
                let row = |title: &str, options: &[&str], selected: Option<usize>, enabled| {
                    SheetRowState {
                        title: title.into(),
                        options: options.iter().map(|o| o.to_string()).collect(),
                        selected,
                        enabled,
                        lit: Vec::new(),
                    }
                };
                let tabs = || {
                    [
                        "CAMERA", "AUDIO", "ASSIST", "LINK", "CONTROLS", "DISPLAY", "STORAGE",
                        "OUTPUT", "SYSTEM",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect()
                };
                if name == "output" {
                    return SheetState {
                        title: "SETTINGS".into(),
                        tabs: tabs(),
                        tab: 7,
                        rows: vec![
                            row("Platform", &["Linux · v4l2loopback"], None, true),
                            row("Camera component", &["Installed"], None, true),
                            row("Detail", &["/dev/video10 · module loaded"], None, true),
                            row("Component", &["Install", "Remove"], None, true),
                            row(
                                "Virtual camera",
                                &["Off", "Camera device", "Stream"],
                                Some(1),
                                true,
                            ),
                            row("Camera picture", &["As shown", "Clean"], Some(1), true),
                            row(
                                "Camera output",
                                &["Camera · /dev/video10 · 1280×720 · 412 frames"],
                                None,
                                true,
                            ),
                            row("Stream", &["Open in the browser"], None, false),
                            row(
                                "Stream address",
                                &["http://127.0.0.1:8890/stream"],
                                None,
                                false,
                            ),
                            row(
                                "OBS",
                                &["Media Source · Local File off · format mjpeg · then Start Virtual Camera"],
                                None,
                                false,
                            ),
                        ],
                    };
                }
                if name == "settings" {
                    return SheetState {
                        title: "SETTINGS".into(),
                        tabs: tabs(),
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
