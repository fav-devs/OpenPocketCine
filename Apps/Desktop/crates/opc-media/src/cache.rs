//! The on-disk library: the same layout the phones use, per camera.
//!
//! ```text
//! <cache>/OpenPocketCine/media/<camera>/thumbs/<path with / → _>.jpg
//!                                       files/<path with / → _>          originals
//!                                       play/<playback cache name>       proxies, .mp4
//!                                       index.json                        the catalogue
//!                                       favorites.json                    local stars
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::{self, MediaFile};

#[derive(Debug, Clone)]
pub struct MediaCache {
    root: PathBuf,
}

impl MediaCache {
    /// Under the platform cache directory, or beside the executable when there is none.
    pub fn for_camera(camera_id: &str) -> Self {
        let base = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        Self::at(
            base.join("OpenPocketCine")
                .join("media")
                .join(safe_component(camera_id)),
        )
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn thumb_path(&self, file: &MediaFile) -> PathBuf {
        self.root
            .join("thumbs")
            .join(format!("{}.jpg", flatten(&file.path)))
    }

    pub fn original_path(&self, file: &MediaFile) -> PathBuf {
        self.root.join("files").join(flatten(&file.path))
    }

    /// Where a proxy (or an original opened for preview) lands for the player.
    pub fn play_path(&self, camera_path: &str) -> PathBuf {
        self.root
            .join("play")
            .join(model::playback_cache_file_name(camera_path))
    }

    pub fn has_thumb(&self, file: &MediaFile) -> bool {
        self.thumb_path(file).is_file()
    }

    pub fn has_original(&self, file: &MediaFile) -> bool {
        self.original_path(file).is_file()
    }

    /// The first cached proxy for a clip, if any.
    pub fn cached_proxy(&self, file: &MediaFile) -> Option<PathBuf> {
        file.proxy_paths()
            .into_iter()
            .map(|path| self.play_path(&path))
            .find(|local| local.is_file())
    }

    pub fn write_thumb(&self, file: &MediaFile, jpeg: &[u8]) -> std::io::Result<()> {
        let path = self.thumb_path(file);
        std::fs::create_dir_all(path.parent().unwrap_or(&self.root))?;
        std::fs::write(path, jpeg)
    }

    pub fn read_thumb(&self, file: &MediaFile) -> Option<Vec<u8>> {
        std::fs::read(self.thumb_path(file)).ok()
    }

    pub fn save_index(&self, files: &[MediaFile]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let json = serde_json::to_vec(files).map_err(std::io::Error::other)?;
        std::fs::write(self.root.join("index.json"), json)
    }

    pub fn load_index(&self) -> Vec<MediaFile> {
        std::fs::read(self.root.join("index.json"))
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default()
    }

    pub fn save_favorites(&self, favorites: &HashSet<String>) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let mut sorted: Vec<&String> = favorites.iter().collect();
        sorted.sort();
        let json = serde_json::to_vec(&sorted).map_err(std::io::Error::other)?;
        std::fs::write(self.root.join("favorites.json"), json)
    }

    pub fn load_favorites(&self) -> HashSet<String> {
        std::fs::read(self.root.join("favorites.json"))
            .ok()
            .and_then(|json| serde_json::from_slice::<Vec<String>>(&json).ok())
            .map(|list| list.into_iter().collect())
            .unwrap_or_default()
    }
}

fn flatten(path: &str) -> String {
    path.replace('/', "_")
}

fn safe_component(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "camera".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_matches_the_phones() {
        let cache = MediaCache::at(PathBuf::from("/tmp/opc-test"));
        let file = MediaFile {
            path: "DCIM/DJI_001/DJI_20260814125250_0034_D.MP4".to_string(),
            ..MediaFile::default()
        };
        assert_eq!(
            cache.thumb_path(&file),
            PathBuf::from("/tmp/opc-test/thumbs/DCIM_DJI_001_DJI_20260814125250_0034_D.MP4.jpg")
        );
        assert_eq!(
            cache.original_path(&file),
            PathBuf::from("/tmp/opc-test/files/DCIM_DJI_001_DJI_20260814125250_0034_D.MP4")
        );
        assert_eq!(
            cache.play_path("DCIM/DJI_001/DJI_20260814125250_0034_D.LRF"),
            PathBuf::from("/tmp/opc-test/play/DCIM_DJI_001_DJI_20260814125250_0034_D.mp4")
        );
        assert_eq!(safe_component("Osmo Pocket 3/abc"), "Osmo_Pocket_3_abc");
    }

    #[test]
    fn the_index_and_favourites_round_trip() {
        let dir = std::env::temp_dir().join(format!("opc-media-cache-{}", std::process::id()));
        let cache = MediaCache::at(dir.clone());
        let file = MediaFile {
            path: "DCIM/DJI_001/DJI_1_D.MP4".to_string(),
            handle: 7,
            ..MediaFile::default()
        };
        cache.save_index(std::slice::from_ref(&file)).unwrap();
        assert_eq!(cache.load_index(), vec![file.clone()]);
        let favorites: HashSet<String> = [file.path.clone()].into_iter().collect();
        cache.save_favorites(&favorites).unwrap();
        assert_eq!(cache.load_favorites(), favorites);
        let _ = std::fs::remove_dir_all(dir);
    }
}
