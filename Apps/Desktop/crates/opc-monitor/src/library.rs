//! The library and player screens: what the shell knows about the camera's card and
//! the clip on screen. The window does the fetching; this decides what to show and
//! what a tap means.

use std::collections::{HashMap, HashSet};

use opc_camera::Command;
use opc_chrome::{CellState, LibraryState, PlayerState, SelectionState};
use opc_media::model::duration_label;
use opc_media::{query, LibrarySort, LibraryTab, MediaFile};

/// What the window must do for the library: everything that touches a socket, a
/// file or the clock.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaAction {
    /// Enter playback, list the card, and fetch as the grid asks.
    OpenLibrary,
    /// Leave the library and bring live view back.
    CloseLibrary,
    /// List the card again.
    Refresh,
    Thumb(MediaFile),
    /// Fetch the proxy (or the original) and open the player on it.
    Play(MediaFile),
    /// Fetch the original to the cache.
    Download(MediaFile),
    /// Fetch the still and open the viewer on it.
    Photo(MediaFile),
    PlayerToggle,
    /// Where to go, in milliseconds.
    PlayerSeek(i64),
    /// Back to the library from the player or the viewer.
    ClosePlayer,
}

/// The card as listed, and how the operator is looking at it.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub files: Vec<MediaFile>,
    pub tab: LibraryTab,
    pub sort: LibrarySort,
    pub selected: Option<String>,
    /// Stars the operator set here, over the camera's own.
    pub favorites: HashSet<String>,
    /// Originals on disk.
    pub cached: HashSet<String>,
    /// Proxies on disk.
    pub proxies: HashSet<String>,
    /// Transfers in flight: `(done, total)`.
    pub progress: HashMap<String, (u64, Option<u64>)>,
    /// A word about a file: a failure, or what is on disk.
    pub notes: HashMap<String, String>,
    /// The file whose DELETE was tapped once; the next tap sends it.
    pub delete_armed: Option<String>,
    pub status: String,
    /// Thumbnails already asked for, so a redraw does not ask again.
    pub thumbs_asked: HashSet<String>,
    pub listing: bool,
    /// The paths on screen, in grid order, so a tapped index finds its file.
    pub visible: Vec<String>,
}

impl Library {
    pub fn file(&self, path: &str) -> Option<&MediaFile> {
        self.files.iter().find(|file| file.path == path)
    }

    pub fn selected_file(&self) -> Option<&MediaFile> {
        self.selected.as_deref().and_then(|path| self.file(path))
    }

    /// The grid in the current tab and order.
    pub fn visible_files(&self) -> Vec<&MediaFile> {
        let filtered = query::filtered(&self.files, self.tab, &self.favorites);
        query::sorted(filtered, self.sort)
    }

    fn is_starred(&self, file: &MediaFile) -> bool {
        file.is_starred || self.favorites.contains(&file.path)
    }

    fn meta(&self, file: &MediaFile) -> String {
        let mut parts = Vec::new();
        if file.is_video() {
            parts.push(file.duration_label());
        }
        if let Some(resolution) = &file.resolution {
            parts.push(resolution.clone());
        }
        if let Some(fps) = file.fps {
            parts.push(format!("{fps} fps"));
        }
        let size = file.size_label();
        if !size.is_empty() {
            parts.push(size);
        }
        parts.join(" · ")
    }

    /// What the chrome draws. Also records the grid order for taps.
    pub fn state(&mut self) -> LibraryState {
        let visible: Vec<MediaFile> = self.visible_files().into_iter().cloned().collect();
        self.visible = visible.iter().map(|file| file.path.clone()).collect();
        let cells = visible
            .iter()
            .map(|file| CellState {
                path: file.path.clone(),
                title: file.filename().to_string(),
                meta: if file.is_video() {
                    file.duration_label()
                } else {
                    file.extension()
                },
                is_video: file.is_video(),
                starred: self.is_starred(file),
                cached: self.cached.contains(&file.path),
                selected: self.selected.as_deref() == Some(file.path.as_str()),
            })
            .collect();
        let selection = self.selected_file().map(|file| {
            let progress = self
                .progress
                .get(&file.path)
                .map(|(done, total)| match total {
                    Some(total) if *total > 0 => (*done as f64 / *total as f64) as f32,
                    _ => 0.0,
                });
            let mut note = self.notes.get(&file.path).cloned().unwrap_or_default();
            if note.is_empty() && self.proxies.contains(&file.path) {
                note = "Proxy on disk".to_string();
            }
            SelectionState {
                title: file.filename().to_string(),
                meta: self.meta(file),
                is_video: file.is_video(),
                starred: self.is_starred(file),
                cached: self.cached.contains(&file.path),
                deletable: file.is_deletable(),
                delete_armed: self.delete_armed.as_deref() == Some(file.path.as_str()),
                progress,
                note,
            }
        });
        let status = if self.listing {
            if self.files.is_empty() {
                "Listing the card…".to_string()
            } else {
                format!("{} files · listing more…", self.files.len())
            }
        } else if !self.status.is_empty() {
            self.status.clone()
        } else if self.files.is_empty() {
            "Nothing listed. Refresh to ask the camera again.".to_string()
        } else {
            format!("{} files", self.files.len())
        };
        LibraryState {
            tab: LibraryTab::ALL
                .iter()
                .position(|tab| *tab == self.tab)
                .unwrap_or(0),
            sort_label: self.sort.label().to_string(),
            status,
            cells,
            selection,
        }
    }

    /// The visible files whose thumbnails have not been asked for yet.
    pub fn thumbs_wanted(&mut self) -> Vec<MediaFile> {
        let wanted: Vec<MediaFile> = self
            .visible_files()
            .into_iter()
            .filter(|file| !self.thumbs_asked.contains(&file.path))
            .cloned()
            .collect();
        for file in &wanted {
            self.thumbs_asked.insert(file.path.clone());
        }
        wanted
    }

    /// A star toggled on the selection: the local overlay flips, and the camera is
    /// told when the record carries a handle to tell it with.
    pub fn toggle_favorite(&mut self, counter: u32) -> Option<Command> {
        let path = self.selected.clone()?;
        let on = !self.favorites.contains(&path)
            && !self.file(&path).is_some_and(|file| file.is_starred);
        if on {
            self.favorites.insert(path.clone());
        } else {
            self.favorites.remove(&path);
        }
        let file = self.files.iter_mut().find(|file| file.path == path)?;
        file.is_starred = on;
        let handle = file.favorite_handle();
        (handle != 0).then_some(Command::MediaFavorite {
            handle,
            counter,
            on,
        })
    }

    /// DELETE tapped on the selection: the first tap arms, the second sends. Only a
    /// handle the fit vouched for ever goes on the wire.
    pub fn delete_tapped(&mut self, counter: u32) -> Option<Command> {
        let path = self.selected.clone()?;
        if self.delete_armed.as_deref() != Some(path.as_str()) {
            self.delete_armed = Some(path);
            return None;
        }
        self.delete_armed = None;
        let file = self.file(&path)?.clone();
        if !file.is_deletable() {
            return None;
        }
        self.files.retain(|f| f.path != path);
        self.selected = None;
        self.status = format!("Deleted {}", file.filename());
        Some(Command::MediaDelete {
            handle: file.handle,
            counter,
        })
    }

    pub fn select_index(&mut self, index: usize) {
        if let Some(path) = self.visible.get(index) {
            self.selected = Some(path.clone());
            self.delete_armed = None;
        }
    }
}

/// The clip or still on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    pub file: MediaFile,
    pub playing: bool,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub proxy: bool,
    pub is_photo: bool,
}

impl Player {
    pub fn state(&self, assists: &[&str]) -> PlayerState {
        let progress = if self.duration_ms > 0 {
            (self.position_ms as f64 / self.duration_ms as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        PlayerState {
            title: self.file.filename().to_string(),
            tag: if self.is_photo {
                self.file.extension()
            } else if self.proxy {
                "PROXY 720P".to_string()
            } else {
                "ORIGINAL".to_string()
            },
            position_label: duration_label(self.position_ms / 1000),
            duration_label: duration_label(self.duration_ms / 1000),
            progress,
            playing: self.playing,
            assists: assists.join("  "),
            is_photo: self.is_photo,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(n: u32, video: bool) -> MediaFile {
        MediaFile {
            path: format!(
                "DCIM/DJI_001/DJI_202608141252{n:02}_00{n:02}_D.{}",
                if video { "MP4" } else { "JPG" }
            ),
            handle: 0x4010_4400 + n,
            duration_seconds: if video { 10 + i64::from(n) } else { 0 },
            ..MediaFile::default()
        }
    }

    #[test]
    fn the_grid_follows_the_tab_and_a_tap_finds_its_file() {
        let mut library = Library {
            files: vec![clip(1, true), clip(2, false), clip(3, true)],
            ..Library::default()
        };
        library.tab = LibraryTab::Photos;
        let state = library.state();
        assert_eq!(state.cells.len(), 1);
        assert_eq!(state.cells[0].meta, "JPG");
        library.select_index(0);
        assert!(library
            .selected_file()
            .unwrap()
            .path
            .ends_with("0002_D.JPG"));
        library.tab = LibraryTab::All;
        let state = library.state();
        assert_eq!(state.cells.len(), 3);
        assert_eq!(state.cells[0].meta, "0:13", "newest first");
    }

    #[test]
    fn delete_arms_first_and_sends_only_a_vouched_handle() {
        let mut library = Library {
            files: vec![clip(1, true)],
            ..Library::default()
        };
        library.select_index(0);
        library.state();
        library.select_index(0);
        assert_eq!(library.delete_tapped(1), None);
        assert!(library.state().selection.unwrap().delete_armed);
        assert_eq!(
            library.delete_tapped(1),
            Some(Command::MediaDelete {
                handle: 0x4010_4401,
                counter: 1
            })
        );
        assert!(library.files.is_empty());

        let mut shared = Library {
            files: vec![MediaFile {
                handle_shared: true,
                ..clip(2, true)
            }],
            ..Library::default()
        };
        shared.state();
        shared.select_index(0);
        shared.delete_tapped(1);
        assert_eq!(
            shared.delete_tapped(2),
            None,
            "a shared handle never goes out"
        );
        assert_eq!(shared.files.len(), 1);
    }

    #[test]
    fn a_star_flips_locally_and_tells_the_camera() {
        let mut library = Library {
            files: vec![clip(1, true)],
            ..Library::default()
        };
        library.state();
        library.select_index(0);
        assert_eq!(
            library.toggle_favorite(3),
            Some(Command::MediaFavorite {
                handle: 0x4010_4401,
                counter: 3,
                on: true
            })
        );
        assert!(library.state().cells[0].starred);
        assert_eq!(
            library.toggle_favorite(4),
            Some(Command::MediaFavorite {
                handle: 0x4010_4401,
                counter: 4,
                on: false
            })
        );
    }

    #[test]
    fn thumbnails_are_asked_for_once() {
        let mut library = Library {
            files: vec![clip(1, true), clip(2, true)],
            ..Library::default()
        };
        assert_eq!(library.thumbs_wanted().len(), 2);
        assert!(library.thumbs_wanted().is_empty());
    }

    #[test]
    fn the_player_reads_its_clock_as_the_phones_do() {
        let player = Player {
            file: clip(1, true),
            playing: true,
            position_ms: 9_400,
            duration_ms: 26_000,
            proxy: true,
            is_photo: false,
        };
        let state = player.state(&["LUT"]);
        assert_eq!(state.position_label, "0:09");
        assert_eq!(state.duration_label, "0:26");
        assert_eq!(state.tag, "PROXY 720P");
        assert!((state.progress - 0.3615).abs() < 0.01);
    }
}
