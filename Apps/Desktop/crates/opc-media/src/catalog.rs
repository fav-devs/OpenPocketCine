//! Turning collected chunks into records, through the core.

use crate::model::MediaFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    /// The core refused the blobs.
    Core(i64),
    /// The core's JSON did not read as records.
    Json(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(code) => write!(f, "the core could not decode the catalogue ({code})"),
            Self::Json(reason) => write!(f, "unreadable catalogue: {reason}"),
        }
    }
}

impl std::error::Error for CatalogError {}

/// Decodes one page: the counter-1 blob, the counter-2 blob, and every chunk merged.
pub fn decode_page(
    sd: &[u8],
    internal: &[u8],
    merged: &[u8],
) -> Result<Vec<MediaFile>, CatalogError> {
    if sd.is_empty() && internal.is_empty() && merged.is_empty() {
        return Ok(Vec::new());
    }
    decode_with_core(sd, internal, merged)
}

/// Without the Swift core beside the build there is nothing to decode with; the layout
/// tests never get here.
#[cfg(not(opc_core_linked))]
fn decode_with_core(
    _sd: &[u8],
    _internal: &[u8],
    _merged: &[u8],
) -> Result<Vec<MediaFile>, CatalogError> {
    Err(CatalogError::Core(-1))
}

#[cfg(opc_core_linked)]
fn decode_with_core(
    sd: &[u8],
    internal: &[u8],
    merged: &[u8],
) -> Result<Vec<MediaFile>, CatalogError> {
    let call = |out: *mut u8, capacity: usize| -> i64 {
        // Safety: every slice outlives the call; the core writes at most `capacity`.
        unsafe {
            opc_core_sys::opc_media_decode(
                sd.as_ptr(),
                sd.len(),
                internal.as_ptr(),
                internal.len(),
                merged.as_ptr(),
                merged.len(),
                out,
                capacity,
            )
        }
    };
    let needed = call(std::ptr::null_mut(), 0);
    if needed < 0 {
        return Err(CatalogError::Core(needed));
    }
    let mut json = vec![0u8; needed as usize];
    let written = call(json.as_mut_ptr(), json.len());
    if written < 0 {
        return Err(CatalogError::Core(written));
    }
    json.truncate(written as usize);
    parse_json(&json)
}

/// The core's JSON array of records.
pub fn parse_json(json: &[u8]) -> Result<Vec<MediaFile>, CatalogError> {
    serde_json::from_slice(json).map_err(|error| CatalogError::Json(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cores_json_reads_as_records() {
        let json = br#"[{"cmdHandle":0,"durationSeconds":12,"group":0,"handle":1074807936,
            "handleCandidate":0,"handleShared":false,"isStarred":true,
            "path":"DCIM/DJI_001/DJI_20260814125250_0034_D.MP4","proxyPath":null,
            "resolution":"3840x2160","fps":30,"sizeBytes":123456789,"storage":0,
            "thumbPath":"MISC/THM/DJI_001/DJI_20260814125250_0034_D.scr"}]"#;
        let files = parse_json(json).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].handle, 0x4010_4480);
        assert!(files[0].is_starred);
        assert_eq!(files[0].resolution.as_deref(), Some("3840x2160"));
        assert_eq!(files[0].duration_label(), "0:12");
    }

    #[test]
    fn nothing_collected_is_an_empty_page_not_a_core_call() {
        assert_eq!(decode_page(&[], &[], &[]).unwrap(), Vec::<MediaFile>::new());
    }
}
