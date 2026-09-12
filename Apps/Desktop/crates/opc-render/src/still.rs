//! Writing a rendered frame out as a PNG.
//!
//! A monitor's real output is a window, which this shell does not have yet. Stills are
//! how the decode and the grade get checked in the meantime, and how an operator grabs a
//! frame off a recorded feed.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::renderer::Rgba;

/// Encodes `image` as 8-bit RGBA PNG.
pub fn write_png(path: &Path, image: &Rgba) -> Result<(), String> {
    let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
    encode(BufWriter::new(file), image).map_err(|error| format!("{}: {error}", path.display()))
}

/// Encodes to any sink, which is what makes this testable without a temporary file.
pub fn encode<W: Write>(sink: W, image: &Rgba) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(sink, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.pixels)
}
