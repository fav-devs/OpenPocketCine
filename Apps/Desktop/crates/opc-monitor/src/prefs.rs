//! Where the desktop keeps what the operator set up: a plain `key=value` file next to
//! the LUT folder, read on start and written on every change.
//!
//! Only the operator's own settings live here — the things the body does not report
//! back, the assist toolbar's shape, the setup tabs. Nothing the camera says is
//! remembered; the camera is asked again.

use std::path::{Path, PathBuf};

use crate::luts;
use crate::sheets::Prefs;

/// `<cache>/OpenPocketCine/desktop-prefs.txt`, beside the LUT folder.
pub fn path() -> PathBuf {
    luts::custom_folder()
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        .join("desktop-prefs.txt")
}

impl Prefs {
    /// One `key=value` per line, in a fixed order.
    pub fn to_lines(&self) -> String {
        let mut out = String::new();
        let mut line = |key: &str, value: String| {
            out.push_str(key);
            out.push('=');
            out.push_str(&value);
            out.push('\n');
        };
        line("audio_channel", self.audio_channel.to_string());
        line("vocal_boost", self.vocal_boost.to_string());
        line("fov", self.fov.to_string());
        line("gimbal_speed", self.gimbal_speed.to_string());
        line("timecode", self.timecode.to_string());
        line("ramp", self.ramp.to_string());
        line("countdown_seconds", self.countdown_seconds.to_string());
        line("stick_sensitivity", self.stick_sensitivity.to_string());
        line("gamepad", self.gamepad.to_string());
        line("show_exposure", self.show_exposure.to_string());
        line("show_status", self.show_status.to_string());
        line("show_zoom", self.show_zoom.to_string());
        line("show_pad", self.show_pad.to_string());
        line("show_modes", self.show_modes.to_string());
        line("vcam", self.vcam.to_string());
        line("vcam_clean", self.vcam_clean.to_string());
        line("vcam_port", self.vcam_port.to_string());
        out
    }

    /// The defaults with every line that parses laid over them. A stray or unknown
    /// line is ignored rather than refused, so an older file still loads.
    pub fn from_lines(text: &str) -> Self {
        let mut prefs = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "audio_channel" => {
                    prefs.audio_channel = value.parse().unwrap_or(prefs.audio_channel)
                }
                "vocal_boost" => prefs.vocal_boost = value.parse().unwrap_or(prefs.vocal_boost),
                "fov" => prefs.fov = value.parse().unwrap_or(prefs.fov),
                "gimbal_speed" => prefs.gimbal_speed = value.parse().unwrap_or(prefs.gimbal_speed),
                "timecode" => prefs.timecode = value.parse().unwrap_or(prefs.timecode),
                "ramp" => prefs.ramp = value.parse().unwrap_or(prefs.ramp),
                "countdown_seconds" => {
                    prefs.countdown_seconds = value.parse().unwrap_or(prefs.countdown_seconds)
                }
                "stick_sensitivity" => {
                    prefs.stick_sensitivity = value
                        .parse()
                        .map_or(prefs.stick_sensitivity, |v: u8| v.clamp(1, 5))
                }
                "gamepad" => prefs.gamepad = value.parse().unwrap_or(prefs.gamepad),
                "show_exposure" => prefs.show_exposure = value.parse().unwrap_or(true),
                "show_status" => prefs.show_status = value.parse().unwrap_or(true),
                "show_zoom" => prefs.show_zoom = value.parse().unwrap_or(true),
                "show_pad" => prefs.show_pad = value.parse().unwrap_or(true),
                "show_modes" => prefs.show_modes = value.parse().unwrap_or(true),
                "vcam" => prefs.vcam = value.parse().ok().filter(|v| *v <= 2).unwrap_or(0),
                "vcam_clean" => prefs.vcam_clean = value.parse().unwrap_or(true),
                "vcam_port" => {
                    prefs.vcam_port = value
                        .parse()
                        .ok()
                        .filter(|port| *port != 0)
                        .unwrap_or(opc_vcam::DEFAULT_PORT)
                }
                _ => {}
            }
        }
        prefs
    }
}

/// The saved settings, or `None` when there is no file or it cannot be read.
pub fn load(path: &Path) -> Option<Prefs> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| Prefs::from_lines(&text))
}

/// Writes the settings, creating the folder on the way.
pub fn save(path: &Path, prefs: &Prefs) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(path, prefs.to_lines())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_survive_a_round_trip_and_an_older_file() {
        let mut prefs = Prefs {
            ramp: 2,
            countdown_seconds: 10,
            stick_sensitivity: 2,
            show_pad: false,
            vcam: 2,
            vcam_clean: false,
            vcam_port: 9000,
            ..Prefs::default()
        };
        prefs.timecode = true;
        let back = Prefs::from_lines(&prefs.to_lines());
        assert_eq!(back, prefs);
        // A file from before the setup tabs: what it does not say stays default.
        let older =
            Prefs::from_lines("ramp=1\nnonsense\nstick_sensitivity=9\nvcam=7\nvcam_port=0\n");
        assert_eq!(older.ramp, 1);
        assert_eq!(older.vcam, 0, "an unknown camera mode is off");
        assert_eq!(older.vcam_port, opc_vcam::DEFAULT_PORT, "port 0 is no port");
        assert_eq!(older.stick_sensitivity, 5, "clamped to the phones' range");
        assert!(older.show_zoom);
        assert_eq!(older.countdown_seconds, Prefs::default().countdown_seconds);
    }

    #[test]
    fn a_file_is_written_where_it_is_read() {
        let dir = std::env::temp_dir().join(format!("opc-prefs-{}", std::process::id()));
        let file = dir.join("nested").join("desktop-prefs.txt");
        let prefs = Prefs {
            gamepad: false,
            ..Prefs::default()
        };
        save(&file, &prefs).expect("saved");
        assert_eq!(load(&file), Some(prefs));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
