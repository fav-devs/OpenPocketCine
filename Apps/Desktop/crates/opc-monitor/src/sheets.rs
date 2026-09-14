//! The sheets: what each one lists, and what a pick on it means.
//!
//! A sheet is built fresh from the body's status every time the chrome redraws, so a
//! chip lights up when the camera confirms the value, not when the operator taps it.
//! Alongside each row of chips is the pick each chip stands for, so a tap is a lookup
//! rather than a second interpretation of the same list.

use opc_camera::{frame_rate_fps, resolution_name, Command, Status};
use opc_chrome::{SheetRowState, SheetState};

use crate::shell::{GimbalMode, Toggles};

/// Which sheet is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetKind {
    Format,
    Exposure,
    Settings,
}

/// The settings tabs, in order.
pub const SETTINGS_TABS: [&str; 3] = ["CAMERA", "AUDIO", "ASSIST"];

/// Settings the body does not report back, kept as last commanded, plus the desktop's
/// own overlays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    /// `0x01` mono, `0x02` stereo, `0x03` spatial.
    pub audio_channel: u8,
    pub vocal_boost: u8,
    /// `0x01` wide, `0x05` natural.
    pub fov: u8,
    /// `0x02` slow, `0x01` default, `0x00` fast.
    pub gimbal_speed: u8,
    pub grid: bool,
    pub timecode: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            audio_channel: 0x02,
            vocal_boost: 0x00,
            fov: 0x01,
            gimbal_speed: 0x01,
            grid: false,
            timecode: false,
        }
    }
}

/// What a chip does when tapped.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Put these on the wire, in order.
    Send(Vec<Command>),
    /// Desktop-side assists, handled by the shell's own toggles.
    Zebra,
    Peaking,
    Grade,
    Mirror,
    Grid(bool),
    Timecode(bool),
    GimbalMode(GimbalMode),
    AudioChannel(u8),
    VocalBoost(u8),
    Fov(u8),
    GimbalSpeed(u8),
    /// A chip that is shown but does nothing here yet.
    Nothing,
}

/// Everything a sheet reads to draw itself.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub status: &'a Status,
    pub prefs: Prefs,
    pub toggles: Toggles,
    pub gimbal_mode: GimbalMode,
    /// The body's model id for commands that encode per model, or -1.
    pub model_id: i32,
}

/// A built sheet: what to draw, and what each chip means.
#[derive(Debug, Clone)]
pub struct Built {
    pub sheet: SheetState,
    pub picks: Vec<Vec<Pick>>,
}

impl Built {
    /// The pick behind a chip, if the row and option exist.
    pub fn pick(&self, row: usize, option: usize) -> Option<&Pick> {
        self.picks.get(row).and_then(|row| row.get(option))
    }
}

/// ISO index on the wire and the value it means. `0x00` is auto.
const ISO_INDEX: [(u8, &str); 10] = [
    (0x00, "Auto"),
    (0x03, "100"),
    (0x04, "200"),
    (0x05, "400"),
    (0x06, "800"),
    (0x07, "1600"),
    (0x08, "3200"),
    (0x09, "6400"),
    (0x0A, "12800"),
    (0x0B, "25600"),
];

/// Shutter denominators offered when the body has not sent its own list.
const SHUTTER_DEFAULT: [i32; 13] = [
    8000, 4000, 2000, 1000, 500, 250, 200, 120, 100, 60, 50, 30, 25,
];

const COLOR_MODES: [(u8, &str); 6] = [
    (0x3F, "Normal"),
    (0x3C, "HDR"),
    (0x17, "D-Log"),
    (0x41, "D-Log2"),
    (0x3D, "Normal 10-bit"),
    (0x00, "D-Log M"),
];

const WHITE_BALANCE: [(i32, &str); 9] = [
    (0, "Auto"),
    (2800, "2800K"),
    (3200, "3200K"),
    (4000, "4000K"),
    (4500, "4500K"),
    (5000, "5000K"),
    (5600, "5600K"),
    (6500, "6500K"),
    (7500, "7500K"),
];

struct RowBuilder {
    row: SheetRowState,
    picks: Vec<Pick>,
}

impl RowBuilder {
    fn new(title: &str) -> Self {
        Self {
            row: SheetRowState {
                title: title.to_string(),
                options: Vec::new(),
                selected: None,
                enabled: true,
            },
            picks: Vec::new(),
        }
    }

    fn option(mut self, label: impl Into<String>, selected: bool, pick: Pick) -> Self {
        if selected {
            self.row.selected = Some(self.row.options.len());
        }
        self.row.options.push(label.into());
        self.picks.push(pick);
        self
    }

    fn enabled(mut self, enabled: bool) -> Self {
        self.row.enabled = enabled;
        self
    }

    /// A row with one greyed chip that explains why there is nothing to pick.
    fn placeholder(title: &str, note: &str) -> Self {
        Self::new(title)
            .option(note, false, Pick::Nothing)
            .enabled(false)
    }
}

fn assemble(title: &str, tabs: &[&str], tab: usize, rows: Vec<RowBuilder>) -> Built {
    let mut sheet = SheetState {
        title: title.to_string(),
        tabs: tabs.iter().map(|tab| tab.to_string()).collect(),
        tab,
        rows: Vec::with_capacity(rows.len()),
    };
    let mut picks = Vec::with_capacity(rows.len());
    for row in rows {
        sheet.rows.push(row.row);
        picks.push(row.picks);
    }
    Built { sheet, picks }
}

/// Builds the sheet the shell has open.
pub fn build(kind: SheetKind, tab: usize, context: Context) -> Built {
    match kind {
        SheetKind::Format => format(context.status),
        SheetKind::Exposure => exposure(context.status),
        SheetKind::Settings => settings(tab, context),
    }
}

fn format(status: &Status) -> Built {
    let formats = &status.available_formats;
    if formats.is_empty() {
        // A picker that invents its own list offers settings the camera will refuse.
        return assemble(
            "FORMAT",
            &[],
            0,
            vec![
                RowBuilder::placeholder("Resolution", "Waiting for the camera's list"),
                RowBuilder::placeholder("Frame rate", "Waiting for the camera's list"),
            ],
        );
    }
    let current = status.video_resolution.zip(status.video_frame_rate);
    let resolution = current
        .map(|(resolution, _)| resolution)
        .unwrap_or(formats[0].0);

    let mut resolutions: Vec<u8> = Vec::new();
    for (code, _) in formats {
        if !resolutions.contains(code) {
            resolutions.push(*code);
        }
    }
    let rates_for = |wanted: u8| -> Vec<u8> {
        formats
            .iter()
            .filter(|(code, _)| *code == wanted)
            .map(|(_, rate)| *rate)
            .collect()
    };

    let mut resolution_row = RowBuilder::new("Resolution");
    for code in &resolutions {
        // Changing size keeps the rate when that size offers it, else takes its first.
        let rates = rates_for(*code);
        let rate = current
            .map(|(_, rate)| rate)
            .filter(|rate| rates.contains(rate))
            .or_else(|| rates.first().copied())
            .unwrap_or(0);
        let name = resolution_name(*code);
        let label = if name.is_empty() {
            format!("0x{code:02X}")
        } else {
            name.to_string()
        };
        resolution_row = resolution_row.option(
            label,
            *code == resolution,
            Pick::Send(vec![Command::SetVideoFormat {
                resolution: *code,
                frame_rate: rate,
            }]),
        );
    }

    let mut rate_row = RowBuilder::new("Frame rate");
    for rate in rates_for(resolution) {
        let label = frame_rate_fps(rate)
            .map(|fps| fps.to_string())
            .unwrap_or_else(|| format!("0x{rate:02X}"));
        rate_row = rate_row.option(
            label,
            current.is_some_and(|(_, now)| now == rate),
            Pick::Send(vec![Command::SetVideoFormat {
                resolution,
                frame_rate: rate,
            }]),
        );
    }

    assemble("FORMAT", &[], 0, vec![resolution_row, rate_row])
}

fn exposure(status: &Status) -> Built {
    let manual = status.expo_mode == Some(0x04);

    let mode = RowBuilder::new("Mode")
        .option(
            "Auto",
            !manual,
            Pick::Send(vec![Command::SetExpoMode(0x01)]),
        )
        .option(
            "Manual",
            manual,
            Pick::Send(vec![Command::SetExpoMode(0x04)]),
        );

    let offered: Vec<u8> = if status.available_iso.is_empty() {
        ISO_INDEX.iter().map(|(index, _)| *index).collect()
    } else {
        status.available_iso.clone()
    };
    let mut iso = RowBuilder::new("ISO").enabled(manual);
    for (index, label) in ISO_INDEX {
        if index != 0x00 && offered.contains(&index) {
            iso = iso.option(
                label,
                status.iso_index == Some(index),
                Pick::Send(vec![Command::SetIsoIndex(index)]),
            );
        }
    }

    let mut iso_max = RowBuilder::new("ISO max").enabled(!manual);
    for limit in 0x02u8..=0x09 {
        let ceiling = 100 << (limit - 1);
        iso_max = iso_max.option(
            format!("100–{ceiling}"),
            status.iso_limit == Some(limit),
            Pick::Send(vec![Command::SetIsoLimit(limit)]),
        );
    }

    let denominators: Vec<i32> = if status.available_shutter.is_empty() {
        SHUTTER_DEFAULT.to_vec()
    } else {
        status.available_shutter.clone()
    };
    let mut shutter = RowBuilder::new("Shutter").enabled(manual);
    for denominator in denominators {
        shutter = shutter.option(
            format!("1/{denominator}"),
            status.shutter_denominator == Some(denominator),
            Pick::Send(vec![Command::SetShutter(denominator)]),
        );
    }

    let mut ev = RowBuilder::new("EV").enabled(!manual);
    for thirds in -9i32..=9 {
        ev = ev.option(
            format!("{:+.1}", f64::from(thirds) / 3.0),
            status.ev_thirds == Some(thirds),
            Pick::Send(vec![Command::SetEv(thirds)]),
        );
    }

    assemble("EXPOSURE", &[], 0, vec![mode, iso, iso_max, shutter, ev])
}

fn settings(tab: usize, context: Context) -> Built {
    let tab = tab.min(SETTINGS_TABS.len() - 1);
    let rows = match tab {
        0 => camera_rows(context),
        1 => audio_rows(context.prefs),
        _ => assist_rows(context.prefs, context.toggles),
    };
    assemble("SETTINGS", &SETTINGS_TABS, tab, rows)
}

fn camera_rows(context: Context) -> Vec<RowBuilder> {
    let status = context.status;
    let prefs = context.prefs;

    // `0xB1` / `0xB2` are the same modes with the tracking bit set.
    let focus_now = status.focus_mode.map(|code| code & 0x0F);
    let focus = RowBuilder::new("Focus")
        .option(
            "Single",
            focus_now == Some(0x01),
            Pick::Send(vec![Command::SetFocusMode(0x01)]),
        )
        .option(
            "Continuous",
            focus_now == Some(0x02),
            Pick::Send(vec![Command::SetFocusMode(0x02)]),
        );

    let kelvin_now = status.white_balance_kelvin.unwrap_or(0);
    let mut white_balance = RowBuilder::new("White balance");
    for (kelvin, label) in WHITE_BALANCE {
        let command = if kelvin == 0 {
            Command::SetWhiteBalanceAuto { tint: 0 }
        } else {
            Command::SetWhiteBalanceCustom { kelvin, tint: 0 }
        };
        // The body reports the kelvin it settled on; the nearest preset lights up.
        let selected = if kelvin == 0 {
            kelvin_now <= 0
        } else {
            kelvin_now > 0 && (kelvin_now - kelvin).abs() < 250
        };
        white_balance = white_balance.option(label, selected, Pick::Send(vec![command]));
    }

    let offered: Vec<u8> = if status.available_colors.is_empty() {
        vec![0x3F, 0x17, 0x41]
    } else {
        status.available_colors.clone()
    };
    let mut color = RowBuilder::new("Color");
    for (code, label) in COLOR_MODES {
        if offered.contains(&code) {
            color = color.option(
                label,
                status.color_mode == Some(code),
                Pick::Send(vec![Command::SetColorMode {
                    mode: code,
                    model_id: context.model_id,
                }]),
            );
        }
    }

    let fov = RowBuilder::new("Field of view")
        .option("Wide", prefs.fov == 0x01, Pick::Fov(0x01))
        .option("Natural", prefs.fov == 0x05, Pick::Fov(0x05));

    let follow = RowBuilder::new("Gimbal mode")
        .option(
            "Follow",
            context.gimbal_mode == GimbalMode::Follow,
            Pick::GimbalMode(GimbalMode::Follow),
        )
        .option(
            "Tilt locked",
            context.gimbal_mode == GimbalMode::TiltLocked,
            Pick::GimbalMode(GimbalMode::TiltLocked),
        )
        .option(
            "FPV",
            context.gimbal_mode == GimbalMode::Fpv,
            Pick::GimbalMode(GimbalMode::Fpv),
        );

    let speed = RowBuilder::new("Gimbal speed")
        .option("Slow", prefs.gimbal_speed == 0x02, Pick::GimbalSpeed(0x02))
        .option(
            "Default",
            prefs.gimbal_speed == 0x01,
            Pick::GimbalSpeed(0x01),
        )
        .option("Fast", prefs.gimbal_speed == 0x00, Pick::GimbalSpeed(0x00));

    vec![focus, white_balance, color, fov, follow, speed]
}

fn audio_rows(prefs: Prefs) -> Vec<RowBuilder> {
    let channel = RowBuilder::new("Channel")
        .option(
            "Stereo",
            prefs.audio_channel == 0x02,
            Pick::AudioChannel(0x02),
        )
        .option(
            "Mono",
            prefs.audio_channel == 0x01,
            Pick::AudioChannel(0x01),
        )
        .option(
            "Spatial",
            prefs.audio_channel == 0x03,
            Pick::AudioChannel(0x03),
        );
    let vocal = RowBuilder::new("Vocal boost")
        .option("Off", prefs.vocal_boost == 0x00, Pick::VocalBoost(0x00))
        .option("On", prefs.vocal_boost == 0x01, Pick::VocalBoost(0x01));
    // Wind and directional audio share one DSP blob the body must be read for first;
    // the desktop cannot read it yet, so these are shown for parity and greyed.
    let wind = RowBuilder::new("Wind noise reduction")
        .option("Off", false, Pick::Nothing)
        .option("On", false, Pick::Nothing)
        .enabled(false);
    let directional = RowBuilder::new("Directional audio")
        .option("All", false, Pick::Nothing)
        .option("Front", false, Pick::Nothing)
        .option("Front + back", false, Pick::Nothing)
        .enabled(false);
    vec![channel, vocal, wind, directional]
}

fn assist_rows(prefs: Prefs, toggles: Toggles) -> Vec<RowBuilder> {
    let on_off = |title: &str, on: bool, off_pick: Pick, on_pick: Pick| {
        RowBuilder::new(title)
            .option("Off", !on, off_pick)
            .option("On", on, on_pick)
    };
    // Zebra and friends are toggles, so the chip that is not lit is the one that acts.
    let toggle = |title: &str, on: bool, pick: Pick| {
        RowBuilder::new(title)
            .option("Off", !on, if on { pick.clone() } else { Pick::Nothing })
            .option("On", on, if on { Pick::Nothing } else { pick })
    };
    vec![
        RowBuilder::new("Grid")
            .option("Off", !prefs.grid, Pick::Grid(false))
            .option("Thirds", prefs.grid, Pick::Grid(true)),
        toggle("Overexposure alert", toggles.zebra, Pick::Zebra),
        toggle("Focus peaking", toggles.peaking, Pick::Peaking),
        toggle("LUT", toggles.grade, Pick::Grade),
        toggle("Mirror", toggles.mirror, Pick::Mirror),
        on_off(
            "Timecode",
            prefs.timecode,
            Pick::Timecode(false),
            Pick::Timecode(true),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(status: &Status) -> Context<'_> {
        Context {
            status,
            prefs: Prefs::default(),
            toggles: Toggles::default(),
            gimbal_mode: GimbalMode::Follow,
            model_id: -1,
        }
    }

    #[test]
    fn the_format_sheet_offers_only_what_the_body_listed() {
        let status = Status {
            available_formats: vec![(0x0A, 0x03), (0x0A, 0x06), (0x10, 0x03)],
            video_resolution: Some(0x0A),
            video_frame_rate: Some(0x06),
            ..Status::default()
        };
        let built = build(SheetKind::Format, 0, context(&status));
        assert_eq!(built.sheet.rows[0].options, ["1080P", "4K"]);
        assert_eq!(built.sheet.rows[0].selected, Some(0));
        assert_eq!(built.sheet.rows[1].options, ["30", "60"]);
        assert_eq!(built.sheet.rows[1].selected, Some(1));
        // 4K has no 60, so picking it takes the first rate 4K offers.
        assert_eq!(
            built.pick(0, 1),
            Some(&Pick::Send(vec![Command::SetVideoFormat {
                resolution: 0x10,
                frame_rate: 0x03
            }]))
        );
    }

    #[test]
    fn an_empty_format_list_is_a_greyed_note_not_an_invented_list() {
        let status = Status::default();
        let built = build(SheetKind::Format, 0, context(&status));
        assert!(built.sheet.rows.iter().all(|row| !row.enabled));
        assert_eq!(built.pick(0, 0), Some(&Pick::Nothing));
    }

    #[test]
    fn the_exposure_sheet_greys_the_rows_the_mode_does_not_use() {
        let status = Status {
            expo_mode: Some(0x04),
            iso_index: Some(0x05),
            shutter_denominator: Some(60),
            ..Status::default()
        };
        let built = build(SheetKind::Exposure, 0, context(&status));
        let titles: Vec<&str> = built.sheet.rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Mode", "ISO", "ISO max", "Shutter", "EV"]);
        assert!(built.sheet.rows[1].enabled && built.sheet.rows[3].enabled);
        assert!(!built.sheet.rows[2].enabled && !built.sheet.rows[4].enabled);
        assert_eq!(
            built.sheet.rows[1].options[built.sheet.rows[1].selected.unwrap()],
            "400"
        );
        assert_eq!(
            built.sheet.rows[3].options[built.sheet.rows[3].selected.unwrap()],
            "1/60"
        );
        assert_eq!(
            built.pick(0, 0),
            Some(&Pick::Send(vec![Command::SetExpoMode(0x01)]))
        );
    }

    #[test]
    fn a_toggle_row_acts_only_on_the_chip_that_is_not_lit() {
        let status = Status::default();
        let built = build(SheetKind::Settings, 2, context(&status));
        let zebra = built
            .sheet
            .rows
            .iter()
            .position(|row| row.title == "Overexposure alert")
            .unwrap();
        assert_eq!(built.sheet.rows[zebra].selected, Some(0));
        assert_eq!(built.pick(zebra, 0), Some(&Pick::Nothing));
        assert_eq!(built.pick(zebra, 1), Some(&Pick::Zebra));
    }
}
