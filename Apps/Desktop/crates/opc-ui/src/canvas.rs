//! An RGBA image the renderer composites over the picture.
//!
//! Everything the chrome draws goes through here, which means the whole HUD can be
//! asserted pixel by pixel in a unit test — no window, no GPU, no camera.

use crate::font::{self, ADVANCE, GLYPH_HEIGHT, GLYPH_WIDTH};

/// Straight (non-premultiplied) RGBA.
pub type Colour = [u8; 4];

pub const TRANSPARENT: Colour = [0, 0, 0, 0];
pub const WHITE: Colour = [255, 255, 255, 255];
/// The record lamp. Bright enough to find on a lit set.
pub const RECORD: Colour = [255, 48, 48, 255];
/// Dimmed record fill — idle state of the REC button.
pub const RECORD_DIM: Colour = [120, 20, 20, 180];
pub const WARNING: Colour = [255, 196, 0, 255];
/// The plate the chrome sits on, dark and mostly transparent so it never hides the shot.
pub const PLATE: Colour = [0, 0, 0, 140];
/// The full-width status bars — slightly blue-black so they read as deliberate, not burnt.
pub const BAR: Colour = [6, 6, 10, 210];
/// A 1-px separator line at bar edges.
pub const BAR_EDGE: Colour = [255, 255, 255, 22];
/// Subtle top-edge highlight on buttons (top face of a bevel).
pub const BEVEL: Colour = [255, 255, 255, 40];
pub const TRACKING: Colour = [64, 255, 128, 255];

/// A drawable RGBA surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Canvas {
    /// A fully transparent surface.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<Colour> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = ((y * self.width + x) * 4) as usize;
        Some([
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ])
    }

    /// Source-over, so chrome can be layered without each piece knowing the others.
    fn blend(&mut self, x: i64, y: i64, colour: Colour) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let at = ((y as u32 * self.width + x as u32) * 4) as usize;
        let source = u32::from(colour[3]);
        if source == 0 {
            return;
        }
        if source == 255 {
            self.pixels[at..at + 4].copy_from_slice(&colour);
            return;
        }
        for (channel, value) in colour.iter().enumerate().take(3) {
            let over = u32::from(*value) * source;
            let under = u32::from(self.pixels[at + channel]) * (255 - source);
            self.pixels[at + channel] = ((over + under) / 255) as u8;
        }
        let existing = u32::from(self.pixels[at + 3]);
        self.pixels[at + 3] = (source + existing * (255 - source) / 255).min(255) as u8;
    }

    /// A filled rectangle. Negative or oversized bounds are clipped, not rejected.
    pub fn fill(&mut self, x: i64, y: i64, width: u32, height: u32, colour: Colour) {
        for row in 0..i64::from(height) {
            for column in 0..i64::from(width) {
                self.blend(x + column, y + row, colour);
            }
        }
    }

    /// A hollow rectangle of the given thickness, drawn inside the bounds.
    pub fn stroke(
        &mut self,
        x: i64,
        y: i64,
        width: u32,
        height: u32,
        thickness: u32,
        colour: Colour,
    ) {
        let thickness = thickness.max(1);
        if width == 0 || height == 0 {
            return;
        }
        self.fill(x, y, width, thickness, colour);
        self.fill(
            x,
            y + i64::from(height) - i64::from(thickness),
            width,
            thickness,
            colour,
        );
        self.fill(x, y, thickness, height, colour);
        self.fill(
            x + i64::from(width) - i64::from(thickness),
            y,
            thickness,
            height,
            colour,
        );
    }

    /// Draws `text` with its top-left at `(x, y)`. Returns the width drawn.
    pub fn text(&mut self, x: i64, y: i64, text: &str, scale: usize, colour: Colour) -> usize {
        let scale = scale.max(1);
        let mut pen = x;
        for character in text.chars() {
            let columns = font::glyph(character);
            for (column, bits) in columns.iter().enumerate() {
                for row in 0..GLYPH_HEIGHT {
                    if bits & (1 << row) == 0 {
                        continue;
                    }
                    self.fill(
                        pen + (column * scale) as i64,
                        y + (row * scale) as i64,
                        scale as u32,
                        scale as u32,
                        colour,
                    );
                }
            }
            pen += (ADVANCE * scale) as i64;
        }
        (pen - x).max(0) as usize
    }

    /// Text on a dark plate, so it stays readable over a bright shot.
    pub fn label(&mut self, x: i64, y: i64, text: &str, scale: usize, colour: Colour) -> u32 {
        let scale = scale.max(1);
        let padding = (2 * scale) as i64;
        let width = font::text_width(text, scale) as u32;
        let height = font::text_height(scale) as u32;
        self.fill(
            x - padding,
            y - padding,
            width + 2 * padding as u32,
            height + 2 * padding as u32,
            PLATE,
        );
        self.text(x, y, text, scale, colour);
        width + 2 * padding as u32
    }

    /// Filled circle using horizontal scan lines.
    pub fn circle(&mut self, cx: i64, cy: i64, r: i64, colour: Colour) {
        if r <= 0 {
            return;
        }
        let r2 = r * r;
        for dy in -r..=r {
            let dx = ((r2 - dy * dy) as f64).sqrt() as i64;
            self.fill(cx - dx, cy + dy, (2 * dx + 1) as u32, 1, colour);
        }
    }

    /// Ring (hollow circle) with the given outer and inner radii.
    pub fn ring(&mut self, cx: i64, cy: i64, outer_r: i64, inner_r: i64, colour: Colour) {
        if outer_r <= 0 {
            return;
        }
        let outer_r2 = outer_r * outer_r;
        let inner_r2 = inner_r * inner_r;
        for dy in -outer_r..=outer_r {
            let outer_dx2 = outer_r2 - dy * dy;
            if outer_dx2 < 0 {
                continue;
            }
            let outer_dx = (outer_dx2 as f64).sqrt() as i64;
            let inner_dx2 = inner_r2 - dy * dy;
            if inner_dx2 <= 0 {
                // Full row is inside the ring.
                self.fill(cx - outer_dx, cy + dy, (2 * outer_dx + 1) as u32, 1, colour);
            } else {
                let inner_dx = (inner_dx2 as f64).sqrt() as i64;
                let left_w = (outer_dx - inner_dx).max(0) as u32;
                if left_w > 0 {
                    self.fill(cx - outer_dx, cy + dy, left_w, 1, colour);
                }
                let right_w = (outer_dx - inner_dx).max(0) as u32;
                if right_w > 0 {
                    self.fill(cx + inner_dx + 1, cy + dy, right_w, 1, colour);
                }
            }
        }
    }

    /// Filled rounded rectangle.
    pub fn rounded_rect(&mut self, x: i64, y: i64, w: u32, h: u32, r: u32, colour: Colour) {
        if w == 0 || h == 0 {
            return;
        }
        let r = (r as i64).min(w as i64 / 2).min(h as i64 / 2);
        let r2 = r * r;
        for row in 0..h as i64 {
            let (lx, rx) = if row < r {
                let dy = r - row;
                let dx = ((r2 - dy * dy).max(0) as f64).sqrt() as i64;
                (x + r - dx, x + w as i64 - r + dx)
            } else if row >= h as i64 - r {
                let dy = row - (h as i64 - r - 1);
                let dx = ((r2 - dy * dy).max(0) as f64).sqrt() as i64;
                (x + r - dx, x + w as i64 - r + dx)
            } else {
                (x, x + w as i64)
            };
            if rx > lx {
                self.fill(lx, y + row, (rx - lx) as u32, 1, colour);
            }
        }
    }

    /// True when nothing has been drawn.
    pub fn is_blank(&self) -> bool {
        self.pixels.chunks_exact(4).all(|pixel| pixel[3] == 0)
    }
}

/// How wide one glyph is at `scale`, for callers laying chrome out by hand.
pub fn glyph_width(scale: usize) -> usize {
    GLYPH_WIDTH * scale.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_canvas_is_empty() {
        let canvas = Canvas::new(16, 16);
        assert!(canvas.is_blank());
        assert_eq!(canvas.pixel(0, 0), Some(TRANSPARENT));
        assert_eq!(canvas.pixel(16, 0), None, "out of bounds reads nothing");
    }

    #[test]
    fn a_fill_lands_where_it_was_asked_to() {
        let mut canvas = Canvas::new(16, 16);
        canvas.fill(4, 4, 2, 2, WHITE);
        assert_eq!(canvas.pixel(4, 4), Some(WHITE));
        assert_eq!(canvas.pixel(5, 5), Some(WHITE));
        assert_eq!(canvas.pixel(6, 6), Some(TRANSPARENT));
        assert_eq!(canvas.pixel(3, 3), Some(TRANSPARENT));
    }

    #[test]
    fn drawing_off_the_edge_clips_instead_of_panicking() {
        let mut canvas = Canvas::new(8, 8);
        canvas.fill(-4, -4, 6, 6, WHITE);
        canvas.fill(6, 6, 40, 40, WHITE);
        assert_eq!(canvas.pixel(0, 0), Some(WHITE));
        assert_eq!(canvas.pixel(7, 7), Some(WHITE));
    }

    #[test]
    fn a_translucent_plate_darkens_rather_than_replaces() {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill(0, 0, 4, 4, WHITE);
        canvas.fill(0, 0, 4, 4, [0, 0, 0, 128]);
        let pixel = canvas.pixel(0, 0).expect("a pixel");
        assert!(
            pixel[0] > 0 && pixel[0] < 255,
            "should be grey, got {pixel:?}"
        );
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn a_stroke_is_hollow() {
        let mut canvas = Canvas::new(16, 16);
        canvas.stroke(2, 2, 10, 10, 1, TRACKING);
        assert_eq!(canvas.pixel(2, 2), Some(TRACKING), "top-left corner");
        assert_eq!(canvas.pixel(11, 11), Some(TRACKING), "bottom-right corner");
        assert_eq!(
            canvas.pixel(6, 6),
            Some(TRANSPARENT),
            "the middle stays clear"
        );
    }

    #[test]
    fn text_draws_something_and_advances() {
        let mut canvas = Canvas::new(64, 16);
        let width = canvas.text(1, 1, "REC", 1, RECORD);
        assert_eq!(width, 3 * ADVANCE);
        assert!(!canvas.is_blank(), "text should mark the canvas");
    }

    #[test]
    fn a_space_marks_nothing_but_still_advances() {
        let mut canvas = Canvas::new(64, 16);
        let width = canvas.text(1, 1, "   ", 1, WHITE);
        assert_eq!(width, 3 * ADVANCE);
        assert!(canvas.is_blank());
    }

    #[test]
    fn scaling_makes_the_same_letter_bigger() {
        let mut small = Canvas::new(64, 32);
        small.text(0, 0, "A", 1, WHITE);
        let mut large = Canvas::new(64, 32);
        large.text(0, 0, "A", 3, WHITE);

        let lit = |canvas: &Canvas| {
            canvas
                .pixels
                .chunks_exact(4)
                .filter(|pixel| pixel[3] > 0)
                .count()
        };
        assert!(
            lit(&large) > lit(&small) * 5,
            "a 3x glyph covers about nine times the area"
        );
    }

    #[test]
    fn a_label_puts_a_plate_behind_its_text() {
        let mut canvas = Canvas::new(64, 32);
        canvas.label(8, 8, "ISO", 1, WHITE);
        // The plate extends past the text, so just outside the glyphs is dark, not clear.
        let behind = canvas.pixel(7, 7).expect("a pixel");
        assert!(behind[3] > 0, "the plate should cover the padding");
        assert!(behind[0] < 64, "and it should be dark");
    }

    #[test]
    fn a_label_reports_the_width_it_used() {
        let mut canvas = Canvas::new(128, 32);
        let width = canvas.label(8, 8, "ISO", 2, WHITE);
        assert_eq!(width, font::text_width("ISO", 2) as u32 + 8);
    }
}
