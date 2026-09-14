//! The LUT menu: the built-in looks the core ships, and the operator's own `.cube`
//! files in a folder. The rules are `CustomLUTIndex` from the core: `.cube` only,
//! case-insensitive order, and no name that could leave the folder.

use std::path::{Path, PathBuf};

/// What the operator picked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LutChoice {
    #[default]
    Off,
    /// One of the core's official Rec.709 cubes, by name.
    BuiltIn(String),
    /// A `.cube` in the custom folder, by file name.
    File(String),
}

/// The names the LUT row offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LutMenu {
    pub builtin: Vec<String>,
    pub custom: Vec<String>,
    /// Where the custom cubes are read from, for the operator to drop files into.
    pub folder: String,
}

/// `<cache>/OpenPocketCine/luts`, beside the media cache.
pub fn custom_folder() -> PathBuf {
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
    base.join("OpenPocketCine").join("luts")
}

/// Keeps only `.cube` entries with safe names, sorted case-insensitively.
pub fn stored(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut cubes: Vec<String> = names
        .into_iter()
        .filter(|name| name.to_lowercase().ends_with(".cube") && is_safe_file_name(name))
        .collect();
    cubes.sort_by_key(|name| name.to_lowercase());
    cubes
}

/// The file names in the custom folder, by the rules above. A missing folder is empty.
pub fn list_custom(folder: &Path) -> Vec<String> {
    let names = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    stored(names)
}

/// The name without a trailing `.cube`, any case.
pub fn display_name(file_name: &str) -> &str {
    if file_name.to_lowercase().ends_with(".cube") {
        &file_name[..file_name.len() - 5]
    } else {
        file_name
    }
}

/// Rejects path components so a hostile name cannot escape the folder.
pub fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains(':')
        && name != "."
        && name != ".."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_safe_cubes_are_listed_in_case_insensitive_order() {
        let names = [
            "Zebra.CUBE",
            "alpha.cube",
            "notes.txt",
            "../escape.cube",
            "sub/dir.cube",
            "Beta.cube",
        ]
        .map(String::from);
        assert_eq!(stored(names), ["alpha.cube", "Beta.cube", "Zebra.CUBE"]);
        assert_eq!(display_name("Beta.cube"), "Beta");
        assert_eq!(display_name("Zebra.CUBE"), "Zebra");
        assert_eq!(display_name("plain"), "plain");
        assert!(!is_safe_file_name("C:evil.cube"));
    }

    #[test]
    fn a_missing_folder_lists_nothing() {
        assert!(list_custom(Path::new("/nowhere/opc-luts")).is_empty());
    }
}
