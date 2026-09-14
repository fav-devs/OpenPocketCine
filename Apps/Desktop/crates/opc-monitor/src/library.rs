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
    /// Count the cache on disk, for the Storage tab.
    CacheSize,
    /// Empty the cache on disk.
    ClearCache,
    /// Playback rate for the conform preview; 1 is the clip's own.
    Speed(f64),
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
    /// Select mode: taps check tiles for a batch delete.
    pub selecting: bool,
    pub checked: HashSet<String>,
    /// The batch delete was tapped once; the next tap sends it.
    pub batch_armed: bool,
    /// Burst groups shown member by member rather than as one tile.
    pub expanded: HashSet<String>,
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
        let visible: Vec<MediaFile> = self.grid_files();
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
                    checked: false,
                    burst: 0,
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
                checked: self.checked.contains(&file.path),
                burst: self.burst_count(file),
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
                burst: self.burst_count(file),
                expanded: file
                    .burst_group_key()
                    .is_some_and(|key| self.expanded.contains(key)),
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
            selecting: self.selecting,
            checked_count: self.checked.len(),
            batch_armed: self.batch_armed,
        }
    }

    /// The tiles to lay out: the visible files with each burst folded under its lead
    /// unless the operator expanded it.
    fn grid_files(&self) -> Vec<MediaFile> {
        let mut out: Vec<MediaFile> = Vec::new();
        let mut leads: HashSet<String> = HashSet::new();
        for file in self.visible_files() {
            match file.burst_group_key() {
                Some(key) if !self.expanded.contains(key) => {
                    if leads.insert(key.to_string()) {
                        out.push(
                            self.burst_lead(key)
                                .cloned()
                                .unwrap_or_else(|| file.clone()),
                        );
                    }
                }
                _ => out.push(file.clone()),
            }
        }
        out
    }

    /// The lowest-numbered member of a burst.
    fn burst_lead(&self, key: &str) -> Option<&MediaFile> {
        self.files
            .iter()
            .filter(|file| file.burst_group_key() == Some(key))
            .min_by_key(|file| file.burst_index())
    }

    /// How many members a folded burst tile stands for; 0 for a plain tile or an
    /// expanded member.
    fn burst_count(&self, file: &MediaFile) -> u32 {
        let Some(key) = file.burst_group_key() else {
            return 0;
        };
        if self.expanded.contains(key) {
            return 0;
        }
        self.files
            .iter()
            .filter(|f| f.burst_group_key() == Some(key))
            .count() as u32
    }

    /// Select mode on or off. Leaving it forgets the checks.
    pub fn toggle_select_mode(&mut self) {
        self.selecting = !self.selecting;
        self.checked.clear();
        self.batch_armed = false;
        self.delete_armed = None;
    }

    /// A tile tapped in select mode: checked or not.
    pub fn toggle_checked(&mut self, index: usize) {
        let Some(path) = self.visible.get(index).filter(|path| !path.is_empty()) else {
            return;
        };
        if !self.checked.remove(path) {
            self.checked.insert(path.clone());
        }
        self.batch_armed = false;
    }

    /// The selection's burst opened up, or folded back.
    pub fn toggle_burst(&mut self) {
        let Some(key) = self
            .selected_file()
            .and_then(|file| file.burst_group_key())
            .map(str::to_string)
        else {
            return;
        };
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
    }

    /// Delete selected tapped: the first tap arms, the second sends one delete per
    /// checked file the fit vouched for, newest counter first.
    pub fn delete_checked(&mut self, mut next_counter: impl FnMut() -> u32) -> Vec<Command> {
        if self.checked.is_empty() {
            return Vec::new();
        }
        if !self.batch_armed {
            self.batch_armed = true;
            return Vec::new();
        }
        self.batch_armed = false;
        let checked = std::mem::take(&mut self.checked);
        let mut commands = Vec::new();
        let mut deleted = 0;
        for path in checked {
            let Some(file) = self.file(&path).cloned() else {
                continue;
            };
            if !file.is_deletable() {
                continue;
            }
            self.files.retain(|f| f.path != path);
            if self.selected.as_deref() == Some(path.as_str()) {
                self.selected = None;
            }
            commands.push(Command::MediaDelete {
                handle: file.handle,
                counter: next_counter(),
            });
            deleted += 1;
        }
        self.status = format!("Deleted {deleted} files");
        self.selecting = false;
        commands
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
    /// The clip's capture rate and the rates it could be conformed to, from the core.
    pub capture_rate: f64,
    pub conform_targets: Vec<f64>,
    /// Which target the preview plays at, if any.
    pub conform: Option<f64>,
}

impl Player {
    /// The conform chip: "120 → 24", or "Conform" when off.
    pub fn conform_label(&self) -> String {
        match self.conform {
            Some(target) => conform_label(self.capture_rate, target),
            None => "Conform".to_string(),
        }
    }

    /// Playback speed for the preview: 1 without a conform.
    pub fn speed(&self) -> f64 {
        self.conform
            .map_or(1.0, |target| conform_speed(self.capture_rate, target))
    }

    /// Cycles off → first target → … → off.
    pub fn next_conform(&mut self) {
        let next = match self.conform {
            None => self.conform_targets.first().copied(),
            Some(current) => self
                .conform_targets
                .iter()
                .position(|t| (t - current).abs() < 1e-9)
                .and_then(|at| self.conform_targets.get(at + 1))
                .copied(),
        };
        self.conform = next;
    }
}

/// The conform targets for a clip, from the core: the rates below its capture rate.
/// None without the core linked.
#[cfg(opc_core_linked)]
pub fn conform_targets(capture_rate: f64, listed_fps: f64) -> Vec<f64> {
    let mut out = [0.0_f64; 8];
    // Safety: `out` has the capacity handed over.
    let count = unsafe {
        opc_core_sys::opc_conform_targets(capture_rate, listed_fps, out.as_mut_ptr(), out.len())
    };
    let count = usize::try_from(count).unwrap_or(0).min(out.len());
    out[..count].to_vec()
}

#[cfg(not(opc_core_linked))]
pub fn conform_targets(_capture_rate: f64, _listed_fps: f64) -> Vec<f64> {
    Vec::new()
}

#[cfg(opc_core_linked)]
pub fn conform_speed(capture_rate: f64, target: f64) -> f64 {
    // Safety: plain values in.
    unsafe { opc_core_sys::opc_conform_speed(capture_rate, target) }
}

#[cfg(not(opc_core_linked))]
pub fn conform_speed(capture_rate: f64, target: f64) -> f64 {
    if capture_rate > 0.0 && target > 0.0 {
        target / capture_rate
    } else {
        1.0
    }
}

#[cfg(opc_core_linked)]
pub fn conform_label(capture_rate: f64, target: f64) -> String {
    // Safety: probing with a null destination only reports the size needed.
    let needed =
        unsafe { opc_core_sys::opc_conform_label(capture_rate, target, std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return String::new();
    }
    let mut bytes = vec![0u8; needed as usize];
    // Safety: `bytes` has exactly the capacity the core asked for.
    let written = unsafe {
        opc_core_sys::opc_conform_label(capture_rate, target, bytes.as_mut_ptr(), bytes.len())
    };
    if written != needed {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(not(opc_core_linked))]
pub fn conform_label(capture_rate: f64, target: f64) -> String {
    format!("{capture_rate:.0} → {target:.0}")
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
            conform_label: self.conform_label(),
            conform_on: self.conform.is_some(),
            conform_available: !self.conform_targets.is_empty(),
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

    fn burst_member(n: u32) -> MediaFile {
        MediaFile {
            path: format!("DCIM/DJI_001/DJI_20260814125205_0005_D_{n:03}.JPG"),
            handle: 0x4010_4500 + n,
            ..MediaFile::default()
        }
    }

    #[test]
    fn a_burst_folds_under_its_lowest_member_until_expanded() {
        let mut library = Library {
            files: vec![
                burst_member(3),
                clip(1, true),
                burst_member(1),
                burst_member(2),
            ],
            ..Library::default()
        };
        let state = library.state(1280.0);
        assert_eq!(
            state.cells.len(),
            3,
            "a header, the clip and one burst tile"
        );
        let (index, tile) = state
            .cells
            .iter()
            .enumerate()
            .find(|(_, cell)| cell.burst > 0)
            .expect("the folded burst");
        assert_eq!(tile.burst, 3);
        assert!(
            tile.path.ends_with("_001.JPG"),
            "the lead is the first frame"
        );

        library.select_index(index);
        let selection = library.state(1280.0).selection.unwrap();
        assert_eq!(selection.burst, 3);
        assert!(!selection.expanded);

        library.toggle_burst();
        let state = library.state(1280.0);
        assert_eq!(state.cells.len(), 5, "every member gets a tile");
        assert!(state.cells.iter().all(|cell| cell.burst == 0));
        assert!(state.selection.unwrap().expanded);

        library.toggle_burst();
        assert_eq!(library.state(1280.0).cells.len(), 3);
    }

    #[test]
    fn select_mode_checks_tiles_and_a_batch_delete_arms_then_sends() {
        let mut library = Library {
            files: vec![
                clip(1, true),
                clip(2, false),
                MediaFile {
                    handle_shared: true,
                    ..clip(3, true)
                },
            ],
            ..Library::default()
        };
        library.state(1280.0);
        let mut counter = 10;
        let mut next = || {
            counter += 1;
            counter
        };
        assert!(
            library.delete_checked(&mut next).is_empty(),
            "nothing checked"
        );

        library.toggle_select_mode();
        library.toggle_checked(0);
        assert!(library.checked.is_empty(), "a header is not a file");
        library.toggle_checked(1);
        library.toggle_checked(2);
        library.toggle_checked(3);
        library.toggle_checked(2);
        library.toggle_checked(2);
        let state = library.state(1280.0);
        assert!(state.selecting);
        assert_eq!(state.checked_count, 3);
        assert_eq!(state.cells.iter().filter(|cell| cell.checked).count(), 3);

        assert!(
            library.delete_checked(&mut next).is_empty(),
            "first tap arms"
        );
        assert!(library.state(1280.0).batch_armed);
        let mut sent = library.delete_checked(&mut next);
        sent.sort_by_key(|command| match command {
            Command::MediaDelete { handle, .. } => *handle,
            _ => 0,
        });
        let handles: Vec<u32> = sent
            .iter()
            .map(|command| match command {
                Command::MediaDelete { handle, .. } => *handle,
                _ => 0,
            })
            .collect();
        assert_eq!(
            handles,
            [0x4010_4401, 0x4010_4402],
            "the shared handle stays"
        );
        let counters: HashSet<u32> = sent
            .iter()
            .map(|command| match command {
                Command::MediaDelete { counter, .. } => *counter,
                _ => 0,
            })
            .collect();
        assert_eq!(counters.len(), 2, "each delete has its own counter");
        assert_eq!(library.files.len(), 1);
        assert!(!library.selecting, "done after the batch");
        assert_eq!(library.status, "Deleted 2 files");

        library.toggle_select_mode();
        library.toggle_checked(1);
        library.toggle_select_mode();
        assert!(
            library.checked.is_empty(),
            "leaving select mode forgets the checks"
        );
    }

    #[test]
    fn the_conform_chip_cycles_the_targets_and_sets_the_speed() {
        let library = Library::default();
        let mut player = Player {
            file: clip(1, true),
            playing: true,
            position_ms: 0,
            duration_ms: 26_000,
            proxy: false,
            is_photo: false,
            show_info: false,
            capture_rate: 120.0,
            conform_targets: vec![24.0, 60.0],
            conform: None,
        };
        let state = player.state(&library, Toggles::default());
        assert_eq!(state.conform_label, "Conform");
        assert!(state.conform_available && !state.conform_on);
        assert_eq!(player.speed(), 1.0);

        player.next_conform();
        assert_eq!(player.conform, Some(24.0));
        assert!((player.speed() - 0.2).abs() < 1e-9);
        let state = player.state(&library, Toggles::default());
        assert!(state.conform_on);
        assert_eq!(state.conform_label, "120 → 24");

        player.next_conform();
        assert_eq!(player.conform, Some(60.0));
        assert!((player.speed() - 0.5).abs() < 1e-9);

        player.next_conform();
        assert_eq!(player.conform, None, "past the last target it is off again");
        assert_eq!(player.speed(), 1.0);

        let mut bare = Player {
            conform_targets: Vec::new(),
            ..player
        };
        bare.next_conform();
        assert_eq!(bare.conform, None);
        assert!(!bare.state(&library, Toggles::default()).conform_available);
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
            capture_rate: 30.0,
            conform_targets: Vec::new(),
            conform: None,
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
