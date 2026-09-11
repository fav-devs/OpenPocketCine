//! Replays a recorded HEVC stream through the decoder and the feed pipeline.
//!
//! This is the one part of the desktop shell that runs without the Swift core, so it is
//! the quickest way to find out whether a given machine can decode and draw at all —
//! before installing a Swift toolchain or having a camera in the room.
//!
//! ```text
//! cargo run -p opc-render --example replay -- feed.h265 stills [--frames 5] [--peaking]
//! ```

use std::path::PathBuf;

use opc_decode::{annexb, Codec, Decoder};
use opc_render::{write_png, FeedRenderer, GradeOptions, Peaking, PeakingSense, Zebra};

fn main() {
    if let Err(message) = run() {
        eprintln!("replay: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = args
        .first()
        .ok_or("usage: replay <file.h265> [out-dir] [--frames N] [--peaking] [--zebra]")?;
    let out = PathBuf::from(args.get(1).map_or("stills", String::as_str));
    let limit = args
        .iter()
        .position(|a| a == "--frames")
        .and_then(|at| args.get(at + 1))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);

    let options = GradeOptions {
        peaking: args.iter().any(|a| a == "--peaking").then_some(Peaking {
            sense: PeakingSense::High,
            color: [1.0, 0.0, 0.0, 1.0],
        }),
        zebra: args.iter().any(|a| a == "--zebra").then(|| Zebra {
            highlight_color: [1.0, 1.0, 1.0, 1.0],
            ..Zebra::highlight(0.9)
        }),
        ..GradeOptions::default()
    };

    let stream =
        std::fs::read(source).map_err(|error| format!("could not read {source}: {error}"))?;
    let units = annexb::access_units(&stream);
    if units.is_empty() {
        return Err(format!("{source} holds no access units"));
    }
    std::fs::create_dir_all(&out)
        .map_err(|error| format!("could not make {}: {error}", out.display()))?;

    let mut decoder = Decoder::new(Codec::Hevc).map_err(|error| error.to_string())?;
    let mut renderer =
        FeedRenderer::new().map_err(|error| format!("{error}. Drawing needs a Vulkan driver."))?;
    println!(
        "{} access units; drawing on {}",
        units.len(),
        renderer.device_name()
    );

    let mut written = 0;
    for unit in units {
        if written >= limit {
            break;
        }
        decoder.send(unit).map_err(|error| error.to_string())?;
        while let Some(picture) = decoder.receive().map_err(|error| error.to_string())? {
            let raster = (picture.width, picture.height);
            let image = renderer
                .render(&picture, raster, options)
                .map_err(|error| error.to_string())?;
            written += 1;
            let path = out.join(format!("frame-{written:05}.png"));
            write_png(&path, &image)?;
            println!("{} — {}x{}", path.display(), image.width, image.height);
            if written >= limit {
                break;
            }
        }
    }
    Ok(())
}
