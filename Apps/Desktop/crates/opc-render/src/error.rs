use std::fmt;

use ash::vk;

/// Why the feed pipeline could not be built or run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// No Vulkan loader, or no ICD the loader would accept.
    NoVulkan(String),
    /// A Vulkan device exists but nothing on it can draw.
    NoGraphicsDevice,
    /// No memory type satisfied a resource's requirements.
    NoSuitableMemory,
    /// The picture had a raster this pipeline cannot take.
    UnsupportedRaster { width: u32, height: u32 },
    /// A Vulkan call failed.
    Vulkan(&'static str, vk::Result),
    /// The core refused a `.cube`, with its own message.
    Lut(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoVulkan(detail) => write!(f, "no usable Vulkan driver: {detail}"),
            Self::NoGraphicsDevice => write!(f, "no Vulkan device on this machine can draw"),
            Self::NoSuitableMemory => write!(f, "no memory type fits the feed resources"),
            Self::UnsupportedRaster { width, height } => {
                write!(f, "cannot draw a {width}x{height} picture")
            }
            Self::Vulkan(operation, result) => write!(f, "{operation} failed: {result:?}"),
            Self::Lut(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Tags a Vulkan result with the call that produced it.
pub(crate) trait Context<T> {
    fn context(self, operation: &'static str) -> Result<T, RenderError>;
}

impl<T> Context<T> for Result<T, vk::Result> {
    fn context(self, operation: &'static str) -> Result<T, RenderError> {
        self.map_err(|result| RenderError::Vulkan(operation, result))
    }
}
