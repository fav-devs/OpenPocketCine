//! A 5x7 bitmap font, rasterised on the CPU.
//!
//! Drawing the chrome in software is a deliberate choice, not a shortcut. It makes the
//! HUD pixel-testable without a GPU, and it keeps the overlay a plain image the renderer
//! composites — so replacing this with a real UI toolkit later means replacing one
//! producer, not unpicking a pipeline.
//!
//! Uppercase only. A field monitor reads better that way, and it halves the table.

/// Glyph width in pixels, before spacing.
pub const GLYPH_WIDTH: usize = 5;
pub const GLYPH_HEIGHT: usize = 7;
/// One blank column between glyphs.
pub const ADVANCE: usize = GLYPH_WIDTH + 1;

/// Column bitmaps, one byte per column, bit 0 at the top.
const GLYPHS: &[(char, [u8; GLYPH_WIDTH])] = &[
    (' ', [0x00, 0x00, 0x00, 0x00, 0x00]),
    ('!', [0x00, 0x00, 0x5F, 0x00, 0x00]),
    ('%', [0x63, 0x13, 0x08, 0x64, 0x63]),
    ('\'', [0x00, 0x00, 0x03, 0x00, 0x00]),
    ('(', [0x00, 0x1C, 0x22, 0x41, 0x00]),
    (')', [0x00, 0x41, 0x22, 0x1C, 0x00]),
    ('*', [0x2A, 0x1C, 0x3E, 0x1C, 0x2A]),
    ('+', [0x08, 0x08, 0x3E, 0x08, 0x08]),
    (',', [0x00, 0x30, 0x70, 0x00, 0x00]),
    ('-', [0x08, 0x08, 0x08, 0x08, 0x08]),
    ('.', [0x00, 0x60, 0x60, 0x00, 0x00]),
    ('/', [0x60, 0x10, 0x08, 0x04, 0x03]),
    ('0', [0x3E, 0x51, 0x49, 0x45, 0x3E]),
    ('1', [0x00, 0x42, 0x7F, 0x40, 0x00]),
    ('2', [0x42, 0x61, 0x51, 0x49, 0x46]),
    ('3', [0x21, 0x41, 0x45, 0x4B, 0x31]),
    ('4', [0x18, 0x14, 0x12, 0x7F, 0x10]),
    ('5', [0x27, 0x45, 0x45, 0x45, 0x39]),
    ('6', [0x3C, 0x4A, 0x49, 0x49, 0x30]),
    ('7', [0x01, 0x71, 0x09, 0x05, 0x03]),
    ('8', [0x36, 0x49, 0x49, 0x49, 0x36]),
    ('9', [0x06, 0x49, 0x49, 0x29, 0x1E]),
    (':', [0x00, 0x36, 0x36, 0x00, 0x00]),
    ('?', [0x02, 0x01, 0x51, 0x09, 0x06]),
    ('A', [0x7E, 0x09, 0x09, 0x09, 0x7E]),
    ('B', [0x7F, 0x49, 0x49, 0x49, 0x36]),
    ('C', [0x3E, 0x41, 0x41, 0x41, 0x22]),
    ('D', [0x7F, 0x41, 0x41, 0x22, 0x1C]),
    ('E', [0x7F, 0x49, 0x49, 0x49, 0x41]),
    ('F', [0x7F, 0x09, 0x09, 0x09, 0x01]),
    ('G', [0x3E, 0x41, 0x41, 0x49, 0x7A]),
    ('H', [0x7F, 0x08, 0x08, 0x08, 0x7F]),
    ('I', [0x00, 0x41, 0x7F, 0x41, 0x00]),
    ('J', [0x30, 0x40, 0x40, 0x40, 0x3F]),
    ('K', [0x7F, 0x08, 0x14, 0x22, 0x41]),
    ('L', [0x7F, 0x40, 0x40, 0x40, 0x40]),
    ('M', [0x7F, 0x02, 0x04, 0x02, 0x7F]),
    ('N', [0x7F, 0x02, 0x04, 0x08, 0x7F]),
    ('O', [0x3E, 0x41, 0x41, 0x41, 0x3E]),
    ('P', [0x7F, 0x09, 0x09, 0x09, 0x06]),
    ('Q', [0x3E, 0x41, 0x51, 0x21, 0x5E]),
    ('R', [0x7F, 0x09, 0x19, 0x29, 0x46]),
    ('S', [0x46, 0x49, 0x49, 0x49, 0x31]),
    ('T', [0x01, 0x01, 0x7F, 0x01, 0x01]),
    ('U', [0x3F, 0x40, 0x40, 0x40, 0x3F]),
    ('V', [0x1F, 0x20, 0x40, 0x20, 0x1F]),
    ('W', [0x7F, 0x20, 0x18, 0x20, 0x7F]),
    ('X', [0x63, 0x14, 0x08, 0x14, 0x63]),
    ('Y', [0x03, 0x04, 0x78, 0x04, 0x03]),
    ('Z', [0x61, 0x51, 0x49, 0x45, 0x43]),
    ('x', [0x44, 0x28, 0x10, 0x28, 0x44]),
];

/// The columns for `character`, falling back to a blank for anything unmapped.
///
/// Lowercase folds to uppercase rather than disappearing, so a caller that forgets is
/// merely shouting instead of silent.
pub fn glyph(character: char) -> [u8; GLYPH_WIDTH] {
    let wanted = character.to_ascii_uppercase();
    for (candidate, columns) in GLYPHS {
        if *candidate == wanted {
            return *columns;
        }
    }
    // An unmapped character is drawn as a blank rather than as a wrong letter.
    [0; GLYPH_WIDTH]
}

/// Width in pixels of `text` at `scale`, including the trailing gap.
pub fn text_width(text: &str, scale: usize) -> usize {
    text.chars().count() * ADVANCE * scale.max(1)
}

pub fn text_height(scale: usize) -> usize {
    GLYPH_HEIGHT * scale.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_listed_once() {
        let mut seen: Vec<char> = GLYPHS.iter().map(|(character, _)| *character).collect();
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), count, "a character is in the table twice");
    }

    #[test]
    fn a_blank_is_blank_and_a_letter_is_not() {
        assert_eq!(glyph(' '), [0; GLYPH_WIDTH]);
        assert_ne!(glyph('A'), [0; GLYPH_WIDTH]);
        assert_ne!(glyph('0'), [0; GLYPH_WIDTH]);
    }

    #[test]
    fn lowercase_shouts_rather_than_vanishing() {
        assert_eq!(glyph('r'), glyph('R'));
    }

    #[test]
    fn an_unmapped_character_draws_nothing() {
        assert_eq!(glyph('\u{2014}'), [0; GLYPH_WIDTH]);
    }

    #[test]
    fn the_digits_are_distinguishable_from_each_other() {
        // A HUD that renders 8 as 0 is worse than one that renders nothing.
        let mut shapes: Vec<[u8; GLYPH_WIDTH]> = "0123456789".chars().map(glyph).collect();
        let count = shapes.len();
        shapes.sort_unstable();
        shapes.dedup();
        assert_eq!(shapes.len(), count);
    }

    #[test]
    fn width_grows_with_the_text_and_the_scale() {
        assert_eq!(text_width("AB", 1), 2 * ADVANCE);
        assert_eq!(text_width("AB", 2), 4 * ADVANCE);
        assert_eq!(text_height(2), GLYPH_HEIGHT * 2);
        assert_eq!(text_width("", 1), 0);
    }
}
