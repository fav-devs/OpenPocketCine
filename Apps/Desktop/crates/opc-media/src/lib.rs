//! The camera's media library, for the desktop shell.
//!
//! The catalogue arrives over the DUML datalink as `0x00/0x27` chunks; the core decodes
//! them (`opc_media_decode`) and this crate reads the result. Files, thumbnails and the
//! 720p proxies come over the camera's SoftAP HTTP server. Nothing here parses a
//! manifest byte: the byte layouts, the Pocket 3's markerless stills and the handle fit
//! that keeps a delete off the neighbouring file all stay in the Swift core, where the
//! phones already exercise them.

pub mod browse;
pub mod cache;
pub mod catalog;
pub mod chunks;
pub mod http;
pub mod model;
pub mod query;
pub mod resume;
pub mod thumbs;
pub mod worker;

pub use browse::{Browse, BrowseEvent, BrowseStep};
pub use cache::MediaCache;
pub use chunks::ChunkAssembler;
pub use http::{MediaHttp, StorageMemo};
pub use model::{MediaFile, MediaKind};
pub use query::{LibrarySort, LibraryTab};
pub use resume::{ResumeAction, ResumePolicy};
pub use worker::{MediaJob, MediaReport, MediaWorker};
