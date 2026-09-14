//! Filtering and sorting the library. No UI, no IO.

use std::collections::HashSet;

use crate::model::{MediaFile, MediaKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibraryTab {
    #[default]
    All,
    Videos,
    Photos,
    Favorites,
}

impl LibraryTab {
    pub const ALL: [LibraryTab; 4] = [Self::All, Self::Videos, Self::Photos, Self::Favorites];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Videos => "VIDEOS",
            Self::Photos => "PHOTOS",
            Self::Favorites => "FAVORITES",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibrarySort {
    #[default]
    Newest,
    Oldest,
    Name,
    Rating,
}

impl LibrarySort {
    pub const ALL: [LibrarySort; 4] = [Self::Newest, Self::Oldest, Self::Name, Self::Rating];

    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "Newest",
            Self::Oldest => "Oldest",
            Self::Name => "Name",
            Self::Rating => "Rating",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Newest => Self::Oldest,
            Self::Oldest => Self::Name,
            Self::Name => Self::Rating,
            Self::Rating => Self::Newest,
        }
    }
}

/// Local favourites overlay the camera's star, the way the phones keep them.
pub fn filtered<'a>(
    files: &'a [MediaFile],
    tab: LibraryTab,
    local_favorites: &HashSet<String>,
) -> Vec<&'a MediaFile> {
    files
        .iter()
        .filter(|file| match tab {
            LibraryTab::All => true,
            LibraryTab::Videos => file.kind() == MediaKind::Video,
            LibraryTab::Photos => file.kind() == MediaKind::Photo,
            LibraryTab::Favorites => file.is_starred || local_favorites.contains(&file.path),
        })
        .collect()
}

pub fn sorted(mut files: Vec<&MediaFile>, order: LibrarySort) -> Vec<&MediaFile> {
    match order {
        LibrarySort::Newest => files.sort_by(|a, b| {
            b.filename_timestamp()
                .unwrap_or("")
                .cmp(a.filename_timestamp().unwrap_or(""))
        }),
        LibrarySort::Oldest => files.sort_by(|a, b| {
            a.filename_timestamp()
                .unwrap_or("")
                .cmp(b.filename_timestamp().unwrap_or(""))
        }),
        LibrarySort::Name => files.sort_by(|a, b| a.filename().cmp(b.filename())),
        LibrarySort::Rating => files.sort_by(|a, b| {
            b.is_starred.cmp(&a.is_starred).then_with(|| {
                b.filename_timestamp()
                    .unwrap_or("")
                    .cmp(a.filename_timestamp().unwrap_or(""))
            })
        }),
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, starred: bool) -> MediaFile {
        MediaFile {
            path: format!("DCIM/DJI_001/{name}"),
            is_starred: starred,
            ..MediaFile::default()
        }
    }

    #[test]
    fn tabs_split_kinds_and_favourites_including_local_ones() {
        let files = vec![
            file("DJI_20260814125250_0034_D.MP4", false),
            file("DJI_20260814125300_0035_D.JPG", true),
            file("DJI_20260814125310_0036_D.MP4", false),
        ];
        let local: HashSet<String> = ["DCIM/DJI_001/DJI_20260814125310_0036_D.MP4".to_string()]
            .into_iter()
            .collect();
        assert_eq!(filtered(&files, LibraryTab::All, &local).len(), 3);
        assert_eq!(filtered(&files, LibraryTab::Videos, &local).len(), 2);
        assert_eq!(filtered(&files, LibraryTab::Photos, &local).len(), 1);
        assert_eq!(filtered(&files, LibraryTab::Favorites, &local).len(), 2);
    }

    #[test]
    fn sorts_read_the_filename_stamp() {
        let files = vec![
            file("DJI_20260814125250_0034_D.MP4", false),
            file("DJI_20260814125310_0036_D.MP4", true),
            file("DJI_20260814125300_0035_D.JPG", false),
        ];
        let all = filtered(&files, LibraryTab::All, &HashSet::new());
        let newest = sorted(all.clone(), LibrarySort::Newest);
        assert_eq!(newest[0].filename(), "DJI_20260814125310_0036_D.MP4");
        let oldest = sorted(all.clone(), LibrarySort::Oldest);
        assert_eq!(oldest[0].filename(), "DJI_20260814125250_0034_D.MP4");
        let rating = sorted(all, LibrarySort::Rating);
        assert!(rating[0].is_starred);
        assert_eq!(LibrarySort::Rating.next(), LibrarySort::Newest);
    }
}
