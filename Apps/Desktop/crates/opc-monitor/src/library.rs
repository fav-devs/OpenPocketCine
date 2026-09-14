//! The library and player screens: what the shell knows about the camera's card and
//! the clip on screen. The window does the fetching; this decides what to show and
//! what a tap means.

use std::collections::{HashMap, HashSet};

use opc_camera::Command;
use opc_chrome::{CellState, LibraryState, PlayerState, SelectionState};
use opc_media::{query, LibrarySort, LibraryTab, MediaFile};

use crate::shell::Toggles;

/// The grid's metrics, in pixels: Mimo's tiles at the camera's own 16:9.
pub const CELL_W: f32 = 220.0;
pub const CELL_H: f32 = 124.0;
pub const GAP: f32 = 10.0;
pub const INSET: f32 = 24.0;
pub const HEADER_H: f32 = 44.0;

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
    /// Local: only what is on this machine, the way Mimo's Local album works.
    pub local: bool,
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
        let mut filtered = query::filtered(&self.files, self.tab, &self.favorites);
        if self.local {
            filtered.retain(|file| {
                self.cached.contains(&file.path) || self.proxies.contains(&file.path)
            });
        }
        query::sorted(filtered, self.sort)
    }

    fn day_title(date_key: &str, today: &str) -> String {
        if date_key.is_empty() {
            "Undated".to_string()
        } else if date_key == today {
            "Today".to_string()
        } else if date_key.len() == 8 {
            format!("{}-{}-{}", &date_key[..4], &date_key[4..6], &date_key[6..])
        } else {
            date_key.to_string()
        }
    }

    pub fn is_starred(&self, file: &MediaFile) -> bool {
        file.is_starred || self.favorites.contains(&file.path)
    }

    pub fn meta(&self, file: &MediaFile) -> String {
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

    /// What the chrome draws for a grid `width` pixels wide: the tiles laid out under
    /// day headers. Also records the cell order for taps (headers are empty slots).
    pub fn state(&mut self, width: f32) -> LibraryState {
        let today = today_key();
        let visible: Vec<MediaFile> = self.visible_files().into_iter().cloned().collect();
        let columns = (((width - 2.0 * INSET + GAP) / (CELL_W + GAP)).floor() as usize).max(1);
        let mut cells: Vec<CellState> = Vec::new();
        let mut order: Vec<String> = Vec::new();
        let mut y = 0.0f32;
        let mut day: Option<String> = None;
        let mut column = 0usize;
        for file in &visible {
            let key = file.date_key().to_string();
            if day.as_deref() != Some(key.as_str()) {
                if day.is_some() {
                    y += CELL_H + GAP;
                }
                cells.push(CellState {
                    path: String::new(),
                    header: true,
                    title: Self::day_title(&key, &today),
                    meta: String::new(),
                    x: 0.0,
                    y,
                    is_video: false,
                    starred: false,
                    cached: false,
                    selected: false,
                });
                order.push(String::new());
                y += HEADER_H + GAP;
                day = Some(key);
                column = 0;
            } else if column == columns {
                column = 0;
                y += CELL_H + GAP;
            }
            cells.push(CellState {
                path: file.path.clone(),
                header: false,
                title: file.filename().to_string(),
                meta: if file.is_video() {
                    file.duration_label()
                } else {
                    file.extension()
                },
                x: column as f32 * (CELL_W + GAP),
                y,
                is_video: file.is_video(),
                starred: self.is_starred(file),
                cached: self.cached.contains(&file.path),
                selected: self.selected.as_deref() == Some(file.path.as_str()),
            });
            order.push(file.path.clone());
            column += 1;
        }
        let content_height = if visible.is_empty() {
            0.0
        } else {
            y + CELL_H + GAP
        };
        self.visible = order;
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
            local: self.local,
            sort_label: self.sort.label().to_string(),
            status,
            cells,
            content_height,
            cell_width: CELL_W,
            cell_height: CELL_H,
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
        if let Some(path) = self.visible.get(index).filter(|path| !path.is_empty()) {
            self.selected = Some(path.clone());
            self.delete_armed = None;
        }
    }
}

/// Today as `YYYYMMDD` in UTC, the same stamp the camera bakes into file names.
pub fn today_key() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(seconds.div_euclid(86_400));
    format!("{y:04}{m:02}{d:02}")
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
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
    pub show_info: bool,
}

/// `mm:ss`, zero-padded like Mimo's time pill.
fn clock_label(ms: i64) -> String {
    let seconds = (ms / 1000).max(0);
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

impl Player {
    pub fn state(&self, library: &Library, toggles: Toggles) -> PlayerState {
        let progress = if self.duration_ms > 0 {
            (self.position_ms as f64 / self.duration_ms as f64).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let file = &self.file;
        PlayerState {
            path: file.path.clone(),
            title: file.filename().to_string(),
            tag: if self.is_photo {
                file.extension()
            } else if self.proxy {
                "Low-Res".to_string()
            } else {
                "Original".to_string()
            },
            info: library.meta(file),
            show_info: self.show_info,
            position_label: clock_label(self.position_ms),
            duration_label: clock_label(self.duration_ms),
            progress,
            playing: self.playing,
            starred: library.is_starred(file),
            cached: library.cached.contains(&file.path),
            deletable: file.is_deletable(),
            delete_armed: library.delete_armed.as_deref() == Some(file.path.as_str()),
            lut_on: toggles.grade,
            zebra_on: toggles.zebra,
            peaking_on: toggles.peaking,
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
        let state = library.state(1280.0);
        assert_eq!(state.cells.len(), 2, "a day header and the tile");
        assert!(state.cells[0].header);
        assert_eq!(state.cells[1].meta, "JPG");
        library.select_index(1);
        assert!(library
            .selected_file()
            .unwrap()
            .path
            .ends_with("0002_D.JPG"));
        library.tab = LibraryTab::All;
        let state = library.state(1280.0);
        assert_eq!(state.cells.len(), 4);
        assert_eq!(state.cells[1].meta, "0:13", "newest first");
        assert_eq!(state.cells[0].title, "2026-08-14");
        // Same day: tiles share the row, laid out left to right.
        assert_eq!(state.cells[1].x, 0.0);
        assert_eq!(state.cells[2].x, CELL_W + GAP);
        assert_eq!(state.cells[1].y, state.cells[2].y);
        assert!(state.content_height > state.cells[3].y);
    }

    #[test]
    fn delete_arms_first_and_sends_only_a_vouched_handle() {
        let mut library = Library {
            files: vec![clip(1, true)],
            ..Library::default()
        };
        library.state(1280.0);
        library.select_index(1);
        assert_eq!(library.delete_tapped(1), None);
        assert!(library.state(1280.0).selection.unwrap().delete_armed);
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
        shared.state(1280.0);
        shared.select_index(1);
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
        library.state(1280.0);
        library.select_index(1);
        assert_eq!(
            library.toggle_favorite(3),
            Some(Command::MediaFavorite {
                handle: 0x4010_4401,
                counter: 3,
                on: true
            })
        );
        assert!(library.state(1280.0).cells[1].starred);
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
    fn local_shows_only_what_is_on_this_machine() {
        let mut library = Library {
            files: vec![clip(1, true), clip(2, true)],
            ..Library::default()
        };
        library.local = true;
        assert!(library.visible_files().is_empty());
        library.proxies.insert(clip(2, true).path);
        assert_eq!(library.visible_files().len(), 1);
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
        let library = Library {
            files: vec![clip(1, true)],
            ..Library::default()
        };
        let player = Player {
            file: clip(1, true),
            playing: true,
            position_ms: 9_400,
            duration_ms: 26_000,
            proxy: true,
            is_photo: false,
            show_info: false,
        };
        let state = player.state(&library, Toggles::default());
        assert_eq!(state.position_label, "00:09");
        assert_eq!(state.duration_label, "00:26");
        assert_eq!(state.tag, "Low-Res");
        assert!((state.progress - 0.3615).abs() < 0.01);
        assert!(state.deletable && !state.starred);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_679), (2026, 8, 14));
    }
}
