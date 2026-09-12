//! What the operator has turned on.
//!
//! These drive push constants and passes that already exist in the Android shell's
//! shaders — nothing here invents a look.

/// Zebra stripes over the ungraded source, the way `feed.frag` paints them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zebra {
    /// Highlight threshold on 0..1, read off the maximum channel. `None` turns it off.
    pub highlight: Option<f32>,
    /// Midtone centre and half-width on 0..1, read off BT.709 luma.
    pub midtone: Option<(f32, f32)>,
    pub highlight_color: [f32; 4],
    pub midtone_color: [f32; 4],
}

impl Default for Zebra {
    fn default() -> Self {
        Self {
            highlight: None,
            midtone: None,
            highlight_color: [1.0, 1.0, 1.0, 1.0],
            midtone_color: [1.0, 1.0, 0.0, 1.0],
        }
    }
}

impl Zebra {
    /// Highlight stripes above `threshold`.
    pub fn highlight(threshold: f32) -> Self {
        Self {
            highlight: Some(threshold),
            ..Self::default()
        }
    }

    /// Midtone stripes in a band around `centre`.
    pub fn midtone(centre: f32, half_width: f32) -> Self {
        Self {
            midtone: Some((centre, half_width)),
            ..Self::default()
        }
    }

    pub(crate) fn is_off(&self) -> bool {
        self.highlight.is_none() && self.midtone.is_none()
    }
}

/// Peaking sensitivity.
///
/// Transcribed from `PeakingSense` in
/// `Apps/Android/app/src/main/kotlin/…/assists/LiveAssistTool.kt`. These numbers are an
/// assist policy and belong in `OpenPocketViewCore` alongside the rest; until they move
/// there, a third shell carrying its own copy is a drift risk worth naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeakingSense {
    Low,
    #[default]
    Medium,
    High,
}

impl PeakingSense {
    pub fn ratio_threshold(self) -> f32 {
        match self {
            Self::Low => 2.30,
            Self::Medium => 2.10,
            Self::High => 1.90,
        }
    }

    pub fn noise_gate(self) -> f32 {
        match self {
            Self::Low => 0.005_22,
            Self::Medium => 0.001_74,
            Self::High => 0.000_58,
        }
    }
}

/// Focus peaking: a closed stroke over the edges the shader finds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peaking {
    pub sense: PeakingSense,
    pub color: [f32; 4],
}

impl Default for Peaking {
    fn default() -> Self {
        Self {
            sense: PeakingSense::default(),
            color: [1.0, 0.0, 0.0, 1.0],
        }
    }
}

/// Everything the feed pipeline needs to know for one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GradeOptions {
    /// Horizontal flip, the MIRROR assist.
    pub mirror: bool,
    /// Bicubic reconstruction when the display raster is larger than the source.
    pub upscale: bool,
    /// Show the cube on half the picture only.
    pub split: bool,
    /// Split down the middle rather than across it.
    pub split_vertical: bool,
    pub zebra: Option<Zebra>,
    pub peaking: Option<Peaking>,
}

impl GradeOptions {
    pub(crate) fn peaking_on(&self) -> bool {
        self.peaking.is_some()
    }
}
