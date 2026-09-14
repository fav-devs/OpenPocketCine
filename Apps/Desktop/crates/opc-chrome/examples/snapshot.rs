//! Render the chrome headlessly over a slate test frame and write PNGs.
//! `cargo run -p opc-chrome --example snapshot -- <out-dir>`
use std::time::Instant;

use opc_chrome::{Chrome, ChromeState};
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
    let (w, h) = (1280u32, 720u32);
    let mut chrome = Chrome::new(Instant::now()).expect("chrome");

    let shots: Vec<(&str, Phase, bool)> = vec![
        ("finding", Phase::Finding, false),
        ("live", Phase::Live, false),
        ("recording", Phase::Live, true),
        ("failed", Phase::Failed("camera went away".into()), false),
    ];

    for (name, phase, rec) in shots {
        let state = ChromeState {
            phase: &phase,
            chip1: "1/50".into(),
            chip2: "ISO 400".into(),
            chip3: "EV 0.0".into(),
            chip4: "5600K".into(),
            link_state: if matches!(phase, Phase::Failed(_)) {
                "OFFLINE"
            } else {
                "LINK"
            },
            is_recording: rec,
            rec_elapsed: "00:12:34".into(),
            battery_text: "82%".into(),
            storage_text: "48G".into(),
            zoom: 2.0,
            zoom_label: "2.0×".into(),
        };
        // Render twice so state changes settle (ReusedBuffer redraws dirty regions).
        let _ = chrome.render(&state, w, h);
        let canvas = chrome.render(&state, w, h);

        // Composite over a slate-gray test frame with a gradient so the overlay is legible.
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let g = 40 + ((x + y) * 60 / (w + h)) as u8;
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
