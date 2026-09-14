//! One catalogue record and everything the shell derives from it.
//!
//! The fields are the core's `MediaFile` as its JSON encoder writes them. The derived
//! rules — kind, deletability, proxy paths, storage, cache names — are transcribed from
//! `MediaManifest.swift` and kept in the same order so a diff against it stays readable.

use serde::{Deserialize, Serialize};

/// The camera's SoftAP address. Every body the shells know serves media here.
pub const HOST: &str = "192.168.2.1";
pub const PORT: u16 = 80;

/// Handles at or above this are videos, and on a two-store body, the internal store.
pub const VIDEO_HANDLE_BASE: u32 = 0x4000_0000;
pub const INTERNAL_BIT: u32 = 0x4000_0000;

/// A file the camera listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaFile {
    /// `DCIM/DJI_001/DJI_20260814125250_0034_D.MP4`.
    pub path: String,
    /// `MISC/THM/DJI_001/DJI_20260814125250_0034_D.scr`, a JPEG.
    pub thumb_path: String,
    /// The delete / favourite handle the fit vouched for, or zero.
    pub handle: u32,
    pub cmd_handle: u32,
    pub size_bytes: u64,
    pub duration_seconds: i64,
    pub is_starred: bool,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub fps: Option<i64>,
    #[serde(default)]
    pub proxy_path: Option<String>,
    /// `/v2?storage=` as the manifest stamped it.
    pub storage: i64,
    pub group: i64,
    /// Two records claimed one handle; neither may be deleted.
    pub handle_shared: bool,
    #[serde(default)]
    pub handle_candidate: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Photo,
}

impl MediaKind {
    pub fn from_extension(extension: &str) -> Self {
        match extension.to_ascii_uppercase().as_str() {
            "JPG" | "JPEG" | "DNG" | "HEIC" | "TIF" | "TIFF" | "PANO" => Self::Photo,
            _ => Self::Video,
        }
    }
}

impl MediaFile {
    pub fn filename(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// Upper-cased, without the dot. Empty when there is none.
    pub fn extension(&self) -> String {
        extension_of(self.filename()).to_ascii_uppercase()
    }

    pub fn kind(&self) -> MediaKind {
        MediaKind::from_extension(&self.extension())
    }

    pub fn is_video(&self) -> bool {
        self.kind() == MediaKind::Video
    }

    /// Only a handle the fit vouched for, and only one record per handle.
    pub fn is_deletable(&self) -> bool {
        self.handle != 0 && !self.handle_shared
    }

    pub fn favorite_handle(&self) -> u32 {
        if self.handle != 0 {
            self.handle
        } else {
            self.cmd_handle
        }
    }

    /// `YYYYMMDDHHmmss` baked into `DJI_20260814125250_0034_D.MP4`.
    pub fn filename_timestamp(&self) -> Option<&str> {
        let name = self.filename();
        let bytes = name.as_bytes();
        let mut start = 0;
        while let Some(offset) = name[start..].find('_') {
            let from = start + offset + 1;
            let to = from + 14;
            if to < bytes.len()
                && bytes[to] == b'_'
                && bytes[from..to].iter().all(u8::is_ascii_digit)
            {
                return Some(&name[from..to]);
            }
            start = from;
        }
        None
    }

    /// `YYYYMMDD`, or empty.
    pub fn date_key(&self) -> &str {
        self.filename_timestamp().map_or("", |stamp| &stamp[..8])
    }

    /// A burst member's group, `DJI_…_0034_D` of `DJI_…_0034_D_003.JPG`, as the core
    /// reads it (`burstRegex`: a `_NNN` before the extension). `None` off a burst.
    pub fn burst_group_key(&self) -> Option<&str> {
        let name = self.filename();
        let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        let (key, index) = stem.rsplit_once('_')?;
        if index.len() == 3 && index.bytes().all(|b| b.is_ascii_digit()) && !key.is_empty() {
            Some(key)
        } else {
            None
        }
    }

    /// The member's place in its burst, or 0 off a burst.
    pub fn burst_index(&self) -> u32 {
        let name = self.filename();
        let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        stem.rsplit_once('_')
            .filter(|(_, index)| index.len() == 3)
            .and_then(|(_, index)| index.parse().ok())
            .unwrap_or(0)
    }

    /// `DJI_…_0034_D` sequence used to fit `base + seq × step`.
    pub fn sequence_number(&self) -> u32 {
        let name = self.filename();
        let bytes = name.as_bytes();
        let mut start = 0;
        while let Some(offset) = name[start..].find('_') {
            let from = start + offset + 1;
            let to = from + 4;
            if to + 1 < bytes.len()
                && &bytes[to..to + 2] == b"_D"
                && bytes[from..to].iter().all(u8::is_ascii_digit)
            {
                return name[from..to].parse().unwrap_or(0);
            }
            start = from;
        }
        0
    }

    /// The listed proxy first, then the derived `.LRF` (DJI) / `.XRF` (CAM_), then the
    /// original: the chain the player tries in order.
    pub fn preview_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        let mut add = |path: String| {
            if !paths.contains(&path) {
                paths.push(path);
            }
        };
        if let Some(proxy) = &self.proxy_path {
            add(proxy.clone());
        }
        if let Some(derived) = self.derived_proxy_path() {
            add(derived);
        }
        add(self.path.clone());
        paths
    }

    /// LRF/XRF sidecars only. Empty for photos and clips with no DJI proxy.
    pub fn proxy_paths(&self) -> Vec<String> {
        self.preview_paths()
            .into_iter()
            .filter(|path| is_proxy_path(path))
            .collect()
    }

    pub fn derived_proxy_path(&self) -> Option<String> {
        let extension = derived_proxy_extension(self.filename())?;
        let stem = match self.path.rfind('.') {
            Some(dot) if dot > self.path.rfind('/').unwrap_or(0) => &self.path[..dot],
            _ => &self.path,
        };
        Some(format!("{stem}.{extension}"))
    }

    /// `(storage, path)` pairs to try when opening a clip: the winning store first,
    /// then the other; proxies first, original last.
    pub fn playback_candidates(&self, first_storage: i64) -> Vec<(i64, String)> {
        let stores = if first_storage == 0 { [0, 1] } else { [1, 0] };
        let mut out = Vec::new();
        for path in self.preview_paths() {
            for storage in stores {
                let pair = (storage, path.clone());
                if !out.contains(&pair) {
                    out.push(pair);
                }
            }
        }
        out
    }

    pub fn duration_label(&self) -> String {
        duration_label(self.duration_seconds)
    }

    pub fn size_label(&self) -> String {
        byte_label(self.size_bytes)
    }
}

fn extension_of(filename: &str) -> &str {
    match filename.rfind('.') {
        Some(dot) => &filename[dot + 1..],
        None => "",
    }
}

pub fn derived_proxy_extension(filename: &str) -> Option<&'static str> {
    if filename.starts_with("CAM_") {
        Some("XRF")
    } else if filename.starts_with("DJI_") {
        Some("LRF")
    } else {
        None
    }
}

pub fn is_proxy_path(path: &str) -> bool {
    matches!(
        extension_of(path).to_ascii_uppercase().as_str(),
        "LRF" | "LRV" | "XRF"
    )
}

/// `http://192.168.2.1/v2?storage=N&path=DCIM/…`. The separators inside `path=` stay
/// as they are: the camera answers `404` to a percent-encoded slash.
pub fn path_url(storage: i64, path: &str) -> String {
    format!("http://{HOST}/v2?storage={storage}&path={path}")
}

/// `/v2?storage=` from the handle's internal bit. Pocket 3 is the single-microSD
/// exception and is always `0`.
pub fn storage_guess(handle: u32, single_sd_storage: bool) -> i64 {
    if single_sd_storage {
        return 0;
    }
    if handle & INTERNAL_BIT != 0 {
        1
    } else {
        0
    }
}

/// The store to ask first for a listed clip. A single-card body is always `0`, even
/// when the manifest stamped `1` from the handle bit or a previous GET remembered `1`.
pub fn resolved_storage(
    stamped: i64,
    handle: u32,
    winner: Option<i64>,
    single_sd_storage: bool,
) -> i64 {
    if single_sd_storage {
        return 0;
    }
    if let Some(winner) = winner.filter(|w| *w == 0 || *w == 1) {
        return winner;
    }
    if stamped == 0 || stamped == 1 {
        return stamped;
    }
    storage_guess(handle, false)
}

/// The on-disk name for a camera path: separators flattened, and a proxy renamed to
/// `.mp4` so a player that keys off the extension opens it.
pub fn playback_cache_file_name(path: &str) -> String {
    let raw = path.replace('/', "_");
    match extension_of(&raw).to_ascii_uppercase().as_str() {
        "MP4" | "MOV" => raw,
        _ => match raw.rfind('.') {
            Some(dot) => format!("{}.mp4", &raw[..dot]),
            None => format!("{raw}.mp4"),
        },
    }
}

/// `/v2` carries no extension, so the type travels out of band.
pub fn playback_mime_type(path: &str) -> &'static str {
    match extension_of(path).to_ascii_uppercase().as_str() {
        "JPG" | "JPEG" => "image/jpeg",
        "DNG" => "image/x-adobe-dng",
        "HEIC" => "image/heic",
        _ => "video/mp4",
    }
}

pub fn duration_label(seconds: i64) -> String {
    if seconds <= 0 {
        return "0:00".to_string();
    }
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn byte_label(bytes: u64) -> String {
    if bytes == 0 {
        return String::new();
    }
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.0} KB");
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{mb:.1} MB");
    }
    format!("{:.2} GB", mb / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(path: &str) -> MediaFile {
        MediaFile {
            path: path.to_string(),
            thumb_path: "MISC/THM/DJI_001/DJI_20260814125250_0034_D.scr".to_string(),
            handle: 0x4010_4480,
            ..MediaFile::default()
        }
    }

    #[test]
    fn a_record_reads_its_name_stamp_and_sequence() {
        let file = clip("DCIM/DJI_001/DJI_20260814125250_0034_D.MP4");
        assert_eq!(file.filename(), "DJI_20260814125250_0034_D.MP4");
        assert_eq!(file.extension(), "MP4");
        assert_eq!(file.kind(), MediaKind::Video);
        assert_eq!(file.filename_timestamp(), Some("20260814125250"));
        assert_eq!(file.date_key(), "20260814");
        assert_eq!(file.sequence_number(), 34);
        assert!(clip("DCIM/DJI_001/DJI_20260814125250_0035_D.JPG").kind() == MediaKind::Photo);
    }

    #[test]
    fn the_proxy_chain_is_listed_then_derived_then_original() {
        let mut file = clip("DCIM/DJI_001/DJI_20260814125250_0034_D.MP4");
        assert_eq!(
            file.preview_paths(),
            [
                "DCIM/DJI_001/DJI_20260814125250_0034_D.LRF",
                "DCIM/DJI_001/DJI_20260814125250_0034_D.MP4"
            ]
        );
        file.proxy_path = Some("DCIM/DJI_001/DJI_20260814125250_0034_D.LRF".to_string());
        assert_eq!(file.proxy_paths().len(), 1);
        let xtra = clip("DCIM/CAM_001/CAM_20260814125250_0034_D.MP4");
        assert_eq!(
            xtra.proxy_paths(),
            ["DCIM/CAM_001/CAM_20260814125250_0034_D.XRF"]
        );
        // The derivation reads the prefix, not the kind, exactly like the core; a still
        // goes through the photo job, which never asks for a proxy.
        assert_eq!(
            clip("DCIM/DJI_001/DJI_20260814125250_0035_D.JPG").proxy_paths(),
            ["DCIM/DJI_001/DJI_20260814125250_0035_D.LRF"]
        );
    }

    #[test]
    fn playback_candidates_try_the_winning_store_first() {
        let file = clip("DCIM/DJI_001/DJI_20260814125250_0034_D.MP4");
        let candidates = file.playback_candidates(1);
        assert_eq!(candidates[0].0, 1);
        assert_eq!(candidates[1].0, 0);
        assert!(candidates[0].1.ends_with(".LRF"));
        assert!(candidates.last().unwrap().1.ends_with(".MP4"));
    }

    #[test]
    fn the_url_keeps_the_slashes_the_camera_wants() {
        assert_eq!(
            path_url(0, "DCIM/DJI_001/DJI_20260814125250_0034_D.MP4"),
            "http://192.168.2.1/v2?storage=0&path=DCIM/DJI_001/DJI_20260814125250_0034_D.MP4"
        );
    }

    #[test]
    fn a_pocket_3_is_always_storage_zero() {
        assert_eq!(storage_guess(0x4010_4480, false), 1);
        assert_eq!(storage_guess(0x4010_4480, true), 0);
        assert_eq!(resolved_storage(1, 0x4010_4480, Some(1), true), 0);
        assert_eq!(resolved_storage(1, 0x4010_4480, Some(0), false), 0);
        assert_eq!(resolved_storage(7, 0x0004_0010, None, false), 0);
    }

    #[test]
    fn cache_names_flatten_and_rename_proxies() {
        assert_eq!(
            playback_cache_file_name("DCIM/DJI_001/DJI_1_D.LRF"),
            "DCIM_DJI_001_DJI_1_D.mp4"
        );
        assert_eq!(
            playback_cache_file_name("DCIM/DJI_001/DJI_1_D.MP4"),
            "DCIM_DJI_001_DJI_1_D.MP4"
        );
        assert_eq!(playback_mime_type("a.LRF"), "video/mp4");
        assert_eq!(playback_mime_type("a.JPG"), "image/jpeg");
    }

    #[test]
    fn labels_read_like_the_phones() {
        assert_eq!(duration_label(0), "0:00");
        assert_eq!(duration_label(75), "1:15");
        assert_eq!(duration_label(3725), "1:02:05");
        assert_eq!(byte_label(0), "");
        assert_eq!(byte_label(512 * 1024), "512 KB");
        assert_eq!(byte_label(3 * 1024 * 1024 + 200 * 1024), "3.2 MB");
        assert_eq!(byte_label(5 * 1024 * 1024 * 1024), "5.00 GB");
    }

    #[test]
    fn deletability_needs_a_vouched_unshared_handle() {
        let mut file = clip("DCIM/DJI_001/DJI_20260814125250_0034_D.MP4");
        assert!(file.is_deletable());
        file.handle_shared = true;
        assert!(!file.is_deletable());
        file.handle = 0;
        file.handle_shared = false;
        file.cmd_handle = 5;
        assert!(!file.is_deletable());
        assert_eq!(file.favorite_handle(), 5);
    }

    #[test]
    fn a_burst_member_names_its_group_and_place() {
        let member = clip("DCIM/DJI_001/DJI_20260814125250_0034_D_003.JPG");
        assert_eq!(member.burst_group_key(), Some("DJI_20260814125250_0034_D"));
        assert_eq!(member.burst_index(), 3);
        assert_eq!(member.date_key(), "20260814");
        let plain = clip("DCIM/DJI_001/DJI_20260814125250_0034_D.MP4");
        assert_eq!(plain.burst_group_key(), None);
        assert_eq!(plain.burst_index(), 0);
        // Only a three-digit suffix is a burst place.
        let four = clip("DCIM/DJI_001/DJI_20260814125250_0034_D_0003.JPG");
        assert_eq!(four.burst_group_key(), None);
    }
}
