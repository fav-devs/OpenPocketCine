//! The assist toolbar and each tool's options, as the phones lay them out.
//!
//! Transcribed from `LiveAssistTool.kt` and the iOS `Assists/` folder: the same fifteen
//! tools in the same groups, with the same option labels. Nothing here decides what a
//! zone looks like — the false-colour lattices and the zebra axis come from the core
//! (see `opc_render::Lut::false_color` and `opc_render::assist_scalars`). This is the
//! menu, not the colour science.

pub use opc_render::{FalseColorScale, PeakingSense};

/// One button on the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssistTool {
    Lut,
    Peak,
    False,
    Zebra,
    Wave,
    Parade,
    Histo,
    Vector,
    Lights,
    Nd,
    Guides,
    Grid,
    Cross,
    Mirror,
    Audio,
}

impl AssistTool {
    /// The toolbar, left to right: `LUT PEAK FALSE | ZEBRA WAVE PARADE | HISTO VECTOR
    /// LIGHTS ND | GUIDES GRID CROSS | MIRROR | AUDIO`.
    pub const TOOLBAR: [Self; 15] = [
        Self::Lut,
        Self::Peak,
        Self::False,
        Self::Zebra,
        Self::Wave,
        Self::Parade,
        Self::Histo,
        Self::Vector,
        Self::Lights,
        Self::Nd,
        Self::Guides,
        Self::Grid,
        Self::Cross,
        Self::Mirror,
        Self::Audio,
    ];

    /// The chip text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Lut => "LUT",
            Self::Peak => "PEAK",
            Self::False => "FALSE",
            Self::Zebra => "ZEBRA",
            Self::Wave => "WAVE",
            Self::Parade => "PARADE",
            Self::Histo => "HISTO",
            Self::Vector => "VECTOR",
            Self::Lights => "LIGHTS",
            Self::Nd => "ND",
            Self::Guides => "GUIDES",
            Self::Grid => "GRID",
            Self::Cross => "CROSS",
            Self::Mirror => "MIRROR",
            Self::Audio => "AUDIO",
        }
    }

    /// The sheet title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Lut => "LUT",
            Self::Peak => "Peaking",
            Self::False => "False Color",
            Self::Zebra => "Zebra",
            Self::Wave => "Waveform",
            Self::Parade => "RGB Parade",
            Self::Histo => "Histogram",
            Self::Vector => "Vectorscope",
            Self::Lights => "Traffic Lights",
            Self::Nd => "ND Suggestion",
            Self::Audio => "Audio Levels",
            Self::Guides => "Guides",
            Self::Grid => "Grid",
            Self::Cross => "Crosshair",
            Self::Mirror => "Mirror",
        }
    }

    /// Which toolbar group the chip sits in; a gap is drawn between groups.
    pub fn group(self) -> usize {
        match self {
            Self::Lut | Self::Peak | Self::False => 0,
            Self::Zebra | Self::Wave | Self::Parade => 1,
            Self::Histo | Self::Vector | Self::Lights | Self::Nd => 2,
            Self::Guides | Self::Grid | Self::Cross => 3,
            Self::Mirror => 4,
            Self::Audio => 5,
        }
    }

    /// AUDIO and MIRROR are tap-only; a long press on them shows only help.
    pub fn has_configuration(self) -> bool {
        !matches!(self, Self::Audio | Self::Mirror)
    }

    /// Every tool is on the desktop. Kept so a chip can still be greyed by policy.
    pub fn available(self) -> bool {
        true
    }

    /// The scopes: tools that read the picture rather than paint on it.
    pub fn is_scope(self) -> bool {
        matches!(
            self,
            Self::Wave | Self::Parade | Self::Histo | Self::Vector | Self::Lights | Self::Nd
        )
    }

    /// Help copy for the tools with nothing to configure.
    pub fn help(self) -> &'static str {
        match self {
            Self::Cross => "Tap the toolbar button to show or hide the centre crosshair.",
            Self::Mirror => "Tap the toolbar button to flip the picture left for right.",
            Self::Audio => "Tap the toolbar button to show the audio meters.",
            _ => "",
        }
    }
}

/// The stroke colour focus peaking paints (`PeakingColor` on the phones).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeakingColor {
    White,
    Blue,
    #[default]
    Red,
    Green,
}

impl PeakingColor {
    pub const ALL: [Self; 4] = [Self::White, Self::Blue, Self::Red, Self::Green];

    pub fn label(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Blue => "Blue",
            Self::Red => "Red",
            Self::Green => "Green",
        }
    }

    /// The overlay RGB the phones paint on focused edges.
    pub fn rgba(self) -> [f32; 4] {
        match self {
            Self::White => [246.0 / 255.0, 241.0 / 255.0, 226.0 / 255.0, 1.0],
            Self::Blue => [64.0 / 255.0, 142.0 / 255.0, 1.0, 1.0],
            Self::Red => [1.0, 72.0 / 255.0, 64.0 / 255.0, 1.0],
            Self::Green => [74.0 / 255.0, 220.0 / 255.0, 132.0 / 255.0, 1.0],
        }
    }
}

/// The sensitivity chips, in the order the phones show them.
pub const PEAKING_SENSES: [PeakingSense; 3] =
    [PeakingSense::Low, PeakingSense::Medium, PeakingSense::High];

pub fn sense_label(sense: PeakingSense) -> &'static str {
    match sense {
        PeakingSense::Low => "Low",
        PeakingSense::Medium => "Med",
        PeakingSense::High => "High",
    }
}

/// A zebra stripe colour (`ZebraPaint` on the phones).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZebraPaint {
    White,
    Amber,
    Red,
    Cyan,
    Green,
}

impl ZebraPaint {
    /// What the highlight stripes may be painted in.
    pub const HIGHLIGHT: [Self; 3] = [Self::White, Self::Amber, Self::Red];
    /// What the midtone band may be painted in.
    pub const MIDTONE: [Self; 3] = [Self::Amber, Self::Cyan, Self::Green];

    pub fn label(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Amber => "Amber",
            Self::Red => "Red",
            Self::Cyan => "Cyan",
            Self::Green => "Green",
        }
    }

    pub fn rgba(self) -> [f32; 4] {
        match self {
            Self::White => [1.0, 1.0, 1.0, 1.0],
            Self::Amber => [1.0, 0.72, 0.2, 1.0],
            Self::Red => [1.0, 0.15, 0.15, 1.0],
            Self::Cyan => [0.0, 0.85, 0.9, 1.0],
            Self::Green => [0.2, 0.9, 0.35, 1.0],
        }
    }
}

/// The zebra thresholds the chips offer, in IRE.
pub const ZEBRA_HIGHLIGHT_STEPS: [f32; 6] = [70.0, 80.0, 90.0, 95.0, 99.0, 100.0];
pub const ZEBRA_MIDTONE_STEPS: [f32; 7] = [40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0];

/// The zebra options. Thresholds stay in IRE whatever the units chip says; the units
/// only change how the chips read (`ZebraAssist.Options` on iOS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZebraOptions {
    /// Show thresholds as IRE (true) or as 0–255 codes on the feed.
    pub ire_units: bool,
    pub highlight_on: bool,
    pub highlight_ire: f32,
    pub highlight_color: ZebraPaint,
    pub midtone_on: bool,
    pub midtone_ire: f32,
    pub midtone_color: ZebraPaint,
}

impl Default for ZebraOptions {
    fn default() -> Self {
        Self {
            ire_units: true,
            highlight_on: true,
            highlight_ire: 99.0,
            highlight_color: ZebraPaint::White,
            midtone_on: true,
            midtone_ire: 55.0,
            midtone_color: ZebraPaint::Amber,
        }
    }
}

impl ZebraOptions {
    /// How a threshold chip reads: the IRE, or the feed code the core says it lands on.
    pub fn step_label(&self, ire: f32, native: f32) -> String {
        if self.ire_units {
            format!("{ire:.0}")
        } else {
            format!("{:.0}", (native.clamp(0.0, 1.0) * 255.0).round())
        }
    }
}

/// Where each threshold chip lands on the feed, from the core. Filled by the shell so
/// the sheet can label chips in 0–255 without a colour-science call per chip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZebraSteps {
    pub highlight: [f32; 6],
    pub midtone: [f32; 7],
}

impl Default for ZebraSteps {
    /// A plain 0–100 to 0–1 map, for when the core is not linked.
    fn default() -> Self {
        Self {
            highlight: ZEBRA_HIGHLIGHT_STEPS.map(|ire| ire / 100.0),
            midtone: ZEBRA_MIDTONE_STEPS.map(|ire| ire / 100.0),
        }
    }
}

/// Which lines the grid draws (`GridAssist.Option` on iOS; any mix may be on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridLine {
    Thirds,
    Phi,
    Diagonal,
}

impl GridLine {
    pub const ALL: [Self; 3] = [Self::Thirds, Self::Phi, Self::Diagonal];

    pub fn label(self) -> &'static str {
        match self {
            Self::Thirds => "Thirds",
            Self::Phi => "Phi Grid",
            Self::Diagonal => "Diagonal",
        }
    }
}

/// The phi grid's fractions; thirds are the obvious ones.
pub const PHI_FRACTIONS: [f32; 2] = [0.382, 0.618];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridOptions {
    pub thirds: bool,
    pub phi: bool,
    pub diagonal: bool,
}

impl Default for GridOptions {
    fn default() -> Self {
        Self {
            thirds: true,
            phi: false,
            diagonal: false,
        }
    }
}

impl GridOptions {
    pub fn get(&self, line: GridLine) -> bool {
        match line {
            GridLine::Thirds => self.thirds,
            GridLine::Phi => self.phi,
            GridLine::Diagonal => self.diagonal,
        }
    }

    pub fn set(&mut self, line: GridLine, on: bool) {
        match line {
            GridLine::Thirds => self.thirds = on,
            GridLine::Phi => self.phi = on,
            GridLine::Diagonal => self.diagonal = on,
        }
    }
}

/// The two tabs of the guides sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuideFamily {
    #[default]
    Film,
    Social,
}

impl GuideFamily {
    pub const ALL: [Self; 2] = [Self::Film, Self::Social];

    pub fn label(self) -> &'static str {
        match self {
            Self::Film => "Film",
            Self::Social => "Social",
        }
    }

    pub fn aspects(self) -> &'static [GuideAspect] {
        match self {
            Self::Film => &GuideAspect::FILM,
            Self::Social => &GuideAspect::SOCIAL,
        }
    }
}

/// An aspect-ratio frame (`GuideAspect` on the phones).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuideAspect {
    Cinema276,
    Cinema,
    Cinema235,
    TwoK,
    Wide,
    Hd,
    Euro,
    Imax,
    Academy,
    Vertical,
    Social,
    Square,
    Portrait,
    Feed,
}

impl GuideAspect {
    pub const FILM: [Self; 9] = [
        Self::Cinema276,
        Self::Cinema,
        Self::Cinema235,
        Self::TwoK,
        Self::Wide,
        Self::Hd,
        Self::Euro,
        Self::Imax,
        Self::Academy,
    ];
    pub const SOCIAL: [Self; 6] = [
        Self::Vertical,
        Self::Social,
        Self::Square,
        Self::Portrait,
        Self::Hd,
        Self::Feed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cinema276 => "2.76:1",
            Self::Cinema => "2.39:1",
            Self::Cinema235 => "2.35:1",
            Self::TwoK => "2.00:1",
            Self::Wide => "1.85:1",
            Self::Hd => "16:9",
            Self::Euro => "1.66:1",
            Self::Imax => "1.43:1",
            Self::Academy => "4:3",
            Self::Vertical => "9:16",
            Self::Social => "4:5",
            Self::Square => "1:1",
            Self::Portrait => "2:3",
            Self::Feed => "1.91:1",
        }
    }

    /// Width over height, parsed from the label the way the phones do it.
    pub fn ratio(self) -> f32 {
        let (a, b) = self.label().split_once(':').unwrap_or(("1", "1"));
        let a: f32 = a.parse().unwrap_or(1.0);
        let b: f32 = b.parse().unwrap_or(1.0);
        if b > 0.0 {
            a / b
        } else {
            1.0
        }
    }

    fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// The guides options: the tab, which frames are on, and whether to darken outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuideOptions {
    pub family: GuideFamily,
    /// A bit per [`GuideAspect`], so the options stay `Copy`.
    selected: u16,
    pub mask: bool,
}

impl Default for GuideOptions {
    fn default() -> Self {
        Self {
            family: GuideFamily::Film,
            selected: GuideAspect::Cinema.bit(),
            mask: false,
        }
    }
}

impl GuideOptions {
    pub fn is_selected(&self, aspect: GuideAspect) -> bool {
        self.selected & aspect.bit() != 0
    }

    pub fn toggle(&mut self, aspect: GuideAspect) {
        self.selected ^= aspect.bit();
    }

    /// The frames to draw; 2.39:1 when nothing is picked, as the phones fall back.
    pub fn frames(&self) -> Vec<GuideAspect> {
        let picked: Vec<GuideAspect> = GuideAspect::FILM
            .iter()
            .chain(GuideAspect::SOCIAL.iter())
            .copied()
            .filter(|aspect| self.is_selected(*aspect))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if picked.is_empty() {
            vec![GuideAspect::Cinema]
        } else {
            picked
        }
    }

    /// What the toolbar chip's tooltip would say: one ratio, or how many.
    pub fn summary(&self) -> String {
        let frames = self.frames();
        match frames.as_slice() {
            [one] => one.label().to_string(),
            many => format!("{} ratios", many.len()),
        }
    }
}

impl PartialOrd for GuideAspect {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GuideAspect {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u16).cmp(&(*other as u16))
    }
}

/// A frame of `ratio` centred in `feed` (`x, y, width, height`): letterboxed when the
/// guide is wider than the feed, pillarboxed when narrower.
pub fn guide_rect(feed: (f32, f32, f32, f32), ratio: f32) -> (f32, f32, f32, f32) {
    let (fx, fy, fw, fh) = feed;
    if fw <= 0.0 || fh <= 0.0 || ratio <= 0.0 {
        return feed;
    }
    let (width, height) = if fw / fh > ratio {
        (fh * ratio, fh)
    } else {
        (fw, fw / ratio)
    };
    (
        fx + (fw - width) / 2.0,
        fy + (fh - height) / 2.0,
        width,
        height,
    )
}

/// The false-colour options: which scale, and whether the key is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FalseColorOptions {
    pub scale: FalseColorScale,
    pub reference: bool,
}

impl Default for FalseColorOptions {
    fn default() -> Self {
        Self {
            scale: FalseColorScale::Stops,
            reference: true,
        }
    }
}

/// Every tool's options, together.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AssistOptions {
    pub false_color: FalseColorOptions,
    pub peaking_color: PeakingColor,
    pub peaking_sense: PeakingSense,
    pub zebra: ZebraOptions,
    pub grid: GridOptions,
    pub guides: GuideOptions,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toolbar_is_the_phones_toolbar() {
        let labels: Vec<&str> = AssistTool::TOOLBAR.iter().map(|t| t.label()).collect();
        assert_eq!(
            labels,
            [
                "LUT", "PEAK", "FALSE", "ZEBRA", "WAVE", "PARADE", "HISTO", "VECTOR", "LIGHTS",
                "ND", "GUIDES", "GRID", "CROSS", "MIRROR", "AUDIO"
            ]
        );
        assert!(!AssistTool::Audio.has_configuration());
        assert!(!AssistTool::Mirror.has_configuration());
        assert!(AssistTool::Cross.has_configuration());
    }

    #[test]
    fn a_guide_wider_than_the_feed_letterboxes_and_a_narrower_one_pillarboxes() {
        let feed = (100.0, 50.0, 1600.0, 900.0);
        let (x, y, w, h) = guide_rect(feed, 2.39);
        assert_eq!((x, w), (100.0, 1600.0));
        assert!((h - 1600.0 / 2.39).abs() < 0.01);
        assert!((y - (50.0 + (900.0 - h) / 2.0)).abs() < 0.01);
        let (x, y, w, h) = guide_rect(feed, 9.0 / 16.0);
        assert_eq!((y, h), (50.0, 900.0));
        assert!((w - 900.0 * 9.0 / 16.0).abs() < 0.01);
        assert!((x - (100.0 + (1600.0 - w) / 2.0)).abs() < 0.01);
    }

    #[test]
    fn guides_fall_back_to_scope_when_nothing_is_picked() {
        let mut guides = GuideOptions::default();
        assert_eq!(guides.frames(), [GuideAspect::Cinema]);
        guides.toggle(GuideAspect::Cinema);
        assert_eq!(guides.frames(), [GuideAspect::Cinema], "the fallback");
        guides.toggle(GuideAspect::Hd);
        guides.toggle(GuideAspect::Square);
        assert_eq!(guides.frames(), [GuideAspect::Hd, GuideAspect::Square]);
        assert_eq!(guides.summary(), "2 ratios");
        assert!((GuideAspect::Hd.ratio() - 16.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn zebra_chips_read_in_ire_or_in_feed_codes() {
        let mut zebra = ZebraOptions::default();
        assert_eq!(zebra.step_label(99.0, 0.9), "99");
        zebra.ire_units = false;
        assert_eq!(zebra.step_label(99.0, 0.9), "230");
    }
}
