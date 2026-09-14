//! The desktop feed pipeline.
//!
//! This is the Android shell's Vulkan path with one pass replaced. Android imports the
//! decoder's AHardwareBuffer and converts it with a `VkSamplerYcbcrConversion`; software
//! decode on a PC produces three planes instead, so only that first pass differs. The
//! grade, the cube sampling, and the stretch run the Android shell's own shaders,
//! compiled from where they live so the two cannot drift.
//!
//! Order matters and is the same as the phones': convert to RGB at the source raster,
//! cube at that raster, and only then stretch. Cubing after the upsample blotched D-Log2
//! on Android, and it would here too.

mod device;
mod error;
mod lut;
mod options;
mod present;
mod renderer;
mod resources;
mod still;

pub use error::RenderError;
pub use lut::{
    assist_scalars, built_in_names, false_color_legend, parse_legend, AssistScalars, LegendBand,
    Lut,
};
pub use present::Presented;
pub use renderer::{
    letterbox, FalseColorScale, FeedRenderer, GradeOptions, Peaking, PeakingSense, Rgba, Zebra,
};
pub use still::{encode, write_png};
