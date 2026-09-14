//! What the camera says about itself.
//!
//! The body describes its state in a stream of pushes, and the core already knows how to
//! read every one — including which model encodes a colour mode which way. This is the
//! handle onto that, plus a plain Rust view of the fields a HUD draws.

use std::ffi::c_void;

use opc_core_sys::{self as sys, OpcCameraStatus, OPC_STATUS_LIST_CAP};

use crate::packed::DumlFrame;

/// A value the camera has not reported.
const UNKNOWN: i32 = -1;

/// The camera's current state, as a HUD would show it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    pub battery_percent: Option<i32>,
    pub charging: bool,
    pub docked: bool,
    pub is_recording: bool,
    pub in_playback: bool,
    pub record_elapsed: i32,
    pub record_remaining: i32,
    pub shooting_mode: Option<i32>,
    pub iso: Option<i32>,
    pub iso_index: Option<u8>,
    pub iso_limit: Option<u8>,
    /// Third-stops from zero: -9 is -3.0 EV.
    pub ev_thirds: Option<i32>,
    /// The N in 1/N.
    pub shutter_denominator: Option<i32>,
    pub fps: Option<i32>,
    pub video_resolution: Option<u8>,
    pub video_frame_rate: Option<u8>,
    pub color_mode: Option<u8>,
    pub expo_mode: Option<u8>,
    pub white_balance_kelvin: Option<i32>,
    pub white_balance_tint: Option<i32>,
    pub focus_mode: Option<u8>,
    pub focus_track: Option<u8>,
    pub storage_free_mb: i32,
    pub storage_total_mb: i32,
    /// Hundredths: 250 is 2.5x.
    pub zoom_hundredths: Option<i32>,
    /// The values this body offers. A picker that invents its own list offers settings
    /// the camera will refuse.
    pub available_shutter: Vec<i32>,
    pub available_iso: Vec<u8>,
    pub available_formats: Vec<(u8, u8)>,
    pub available_colors: Vec<u8>,
    pub timecode: Option<String>,
    pub firmware: Option<String>,
    /// The gimbal's last `0x04/0x05` attitude in 0.1°: yaw, display tilt (look-up
    /// positive), and the native absolute pitch timed targets take.
    pub gimbal_yaw_tenth: Option<i16>,
    pub gimbal_pitch_tenth: Option<i16>,
    pub gimbal_native_pitch_tenth: Option<i16>,
    /// Counts attitude pushes, so a shell can tell a fresh reading from a held one.
    pub gimbal_attitude_seq: u32,
    /// Audio DSP `@2` as the core reads it: wind `0x18` off / `0x1A` on; directional
    /// `0xDA` all / `0x3A` front / `0xBA` front+back.
    pub wind_nr: Option<u8>,
    pub directional_audio: Option<u8>,
    /// The body's 26-byte DSP blob, which a wind or directional write carries back
    /// patched. `None` until the `0x02/0xA0` GET has answered.
    pub audio_dsp_blob: Option<[u8; AUDIO_DSP_BLOB]>,
}

/// The audio DSP blob's length on the wire.
pub const AUDIO_DSP_BLOB: usize = 26;

impl Status {
    /// Elapsed recording time as `h:mm:ss`, or `mm:ss` under an hour.
    pub fn elapsed_label(&self) -> String {
        let seconds = self.record_elapsed.max(0);
        let (hours, minutes, seconds) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
        if hours > 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }

    /// Zoom as an operator reads it: `2.5x`.
    pub fn zoom_label(&self) -> Option<String> {
        let hundredths = self.zoom_hundredths?;
        let whole = hundredths / 100;
        let rest = (hundredths % 100) / 10;
        Some(if rest == 0 {
            format!("{whole}x")
        } else {
            format!("{whole}.{rest}x")
        })
    }

    pub fn shutter_label(&self) -> Option<String> {
        self.shutter_denominator
            .map(|denominator| format!("1/{denominator}"))
    }

    /// EV as the operator reads it: `-0.3`, `+1.0`.
    pub fn ev_label(&self) -> Option<String> {
        self.ev_thirds
            .map(|thirds| format!("{:+.1}", f64::from(thirds) / 3.0))
    }

    /// The format chip: `1080P·60`. Resolution and rate are shown when the body has
    /// reported them; nothing is guessed.
    pub fn format_label(&self) -> String {
        let resolution = self.video_resolution.map(resolution_name);
        let rate = self.video_frame_rate.and_then(frame_rate_fps);
        match (resolution, rate) {
            (Some(resolution), Some(rate)) => format!("{resolution}·{rate}"),
            (Some(resolution), None) => resolution.to_string(),
            (None, Some(rate)) => format!("{rate} FPS"),
            (None, None) => String::new(),
        }
    }

    /// Recording time left on the card as `h:mm:ss`, or the free space when the body
    /// has not said how long that is.
    pub fn remaining_label(&self) -> String {
        if self.record_remaining > 0 {
            let seconds = self.record_remaining;
            let (hours, minutes, seconds) = (seconds / 3600, (seconds / 60) % 60, seconds % 60);
            return format!("{hours}:{minutes:02}:{seconds:02}");
        }
        format!("{} GB", self.storage_free_mb / 1024)
    }
}

/// The size half of a video format, from the body's `0x02/0x18` catalogue.
pub fn resolution_name(code: u8) -> &'static str {
    match code {
        0x0A | 0x0C | 0x42 | 0x69 => "1080P",
        0x2D | 0x43 | 0x5F => "2.7K",
        0x10 | 0x67 | 0x7D => "4K",
        0x6A => "2160P",
        0x6B | 0x6C => "3K",
        _ => "",
    }
}

/// Frames per second for a frame-rate index, from the Osmosis table.
pub fn frame_rate_fps(index: u8) -> Option<u32> {
    Some(match index {
        0x01 => 24,
        0x02 => 25,
        0x03 => 30,
        0x04 => 48,
        0x05 => 50,
        0x06 => 60,
        0x07 => 120,
        0x08 => 240,
        0x0A => 100,
        0x0B => 96,
        0x1D => 15,
        _ => return None,
    })
}

fn optional(value: i32) -> Option<i32> {
    (value != UNKNOWN).then_some(value)
}

fn optional_byte(value: i32) -> Option<u8> {
    (value >= 0 && value <= i32::from(u8::MAX)).then_some(value as u8)
}

fn list(values: &[i32; OPC_STATUS_LIST_CAP], count: i32) -> Vec<i32> {
    values[..count.clamp(0, OPC_STATUS_LIST_CAP as i32) as usize].to_vec()
}

/// Folds camera pushes into a current picture of the body.
#[derive(Debug)]
pub struct StatusDecoder {
    handle: *mut c_void,
}

impl StatusDecoder {
    /// `model_id` is the body's own identifier, when the shell knows it. Without one the
    /// decoder falls back to the encodings every Osmo shares, which reads some colour
    /// modes wrong on a Pocket 3 or a Nano.
    pub fn new(model_id: Option<i32>) -> Self {
        // Safety: the core returns a retained handle released in `Drop`.
        Self {
            handle: unsafe { sys::opc_status_create(model_id.unwrap_or(-1)) },
        }
    }

    /// Applies one frame. True when the decoder recognised it as status.
    pub fn apply(&mut self, frame: &DumlFrame) -> bool {
        // Safety: the payload outlives the call.
        unsafe {
            sys::opc_status_apply_frame(
                self.handle,
                frame.sender,
                frame.receiver,
                frame.seq,
                frame.flags,
                frame.cmd_set,
                frame.cmd_id,
                frame.payload.as_ptr(),
                frame.payload.len(),
            ) == 1
        }
    }

    /// Applies a `0x00/0x99` subscription push — timecode and the available-value lists.
    pub fn apply_push(&mut self, payload: &[u8]) -> bool {
        // Safety: `payload` outlives the call.
        unsafe { sys::opc_status_apply_push(self.handle, payload.as_ptr(), payload.len()) == 1 }
    }

    fn text(
        &self,
        read: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> i64,
    ) -> Option<String> {
        // Safety: probing with a null destination only reports the size.
        let needed = unsafe { read(self.handle, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return None;
        }
        let mut bytes = vec![0u8; needed as usize];
        // Safety: `bytes` has exactly the capacity the core asked for.
        let written = unsafe { read(self.handle, bytes.as_mut_ptr(), bytes.len()) };
        if written != needed {
            return None;
        }
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The camera's current state.
    pub fn status(&self) -> Status {
        let mut raw = OpcCameraStatus::default();
        // Safety: `raw` is a live record and the handle is live for `self`.
        unsafe { sys::opc_status_read(self.handle, &mut raw) };

        let resolutions = list(&raw.available_format_resolution, raw.available_format_count);
        let rates = list(&raw.available_format_frame_rate, raw.available_format_count);
        Status {
            battery_percent: optional(raw.battery_percent),
            charging: raw.charging != 0,
            docked: raw.docked != 0,
            is_recording: raw.is_recording != 0,
            in_playback: raw.in_playback != 0,
            record_elapsed: raw.record_elapsed_sec,
            record_remaining: raw.record_remaining_sec,
            shooting_mode: optional(raw.shooting_mode),
            iso: optional(raw.iso),
            iso_index: optional_byte(raw.iso_index),
            iso_limit: optional_byte(raw.iso_limit),
            ev_thirds: (raw.has_ev != 0).then_some(raw.ev_thirds),
            shutter_denominator: optional(raw.shutter_denom),
            fps: (raw.fps > 0).then_some(raw.fps),
            video_resolution: optional_byte(raw.video_resolution),
            video_frame_rate: optional_byte(raw.video_frame_rate),
            color_mode: optional_byte(raw.color_mode),
            expo_mode: optional_byte(raw.expo_mode),
            white_balance_kelvin: optional(raw.white_balance_kelvin),
            white_balance_tint: (raw.has_white_balance_tint != 0).then_some(raw.white_balance_tint),
            focus_mode: optional_byte(raw.focus_mode),
            focus_track: optional_byte(raw.focus_track),
            storage_free_mb: raw.storage_free_mb,
            storage_total_mb: raw.storage_total_mb,
            gimbal_yaw_tenth: (raw.gimbal_attitude_seq > 0).then_some(raw.gimbal_yaw_tenth as i16),
            gimbal_pitch_tenth: (raw.gimbal_attitude_seq > 0)
                .then_some(raw.gimbal_pitch_tenth as i16),
            gimbal_native_pitch_tenth: (raw.gimbal_attitude_seq > 0)
                .then_some(raw.gimbal_native_pitch_tenth as i16),
            gimbal_attitude_seq: raw.gimbal_attitude_seq.max(0) as u32,
            zoom_hundredths: optional(raw.zoom_hundredths),
            wind_nr: optional_byte(raw.wind_nr),
            directional_audio: optional_byte(raw.directional_audio),
            audio_dsp_blob: (raw.audio_dsp_blob_count == AUDIO_DSP_BLOB as i32).then(|| {
                let mut blob = [0u8; AUDIO_DSP_BLOB];
                for (out, value) in blob.iter_mut().zip(raw.audio_dsp_blob.iter()) {
                    *out = (*value).clamp(0, 255) as u8;
                }
                blob
            }),
            available_shutter: list(&raw.available_shutter, raw.available_shutter_count),
            available_iso: list(&raw.available_iso, raw.available_iso_count)
                .into_iter()
                .filter_map(optional_byte)
                .collect(),
            available_formats: resolutions
                .into_iter()
                .zip(rates)
                .filter_map(|(resolution, rate)| {
                    Some((optional_byte(resolution)?, optional_byte(rate)?))
                })
                .collect(),
            available_colors: list(&raw.available_color, raw.available_color_count)
                .into_iter()
                .filter_map(optional_byte)
                .collect(),
            timecode: self
                .text(sys::opc_status_timecode)
                .filter(|text| !text.is_empty()),
            firmware: self
                .text(sys::opc_status_firmware)
                .filter(|text| !text.is_empty()),
        }
    }
}

impl Drop for StatusDecoder {
    fn drop(&mut self) {
        // Safety: retained by `opc_status_create` and released exactly once.
        unsafe { sys::opc_status_destroy(self.handle) }
    }
}

// Plain heap state with no thread affinity; `&mut self` gates every mutating call.
unsafe impl Send for StatusDecoder {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_reads_as_a_clock() {
        assert_eq!(Status::default().elapsed_label(), "0:00");
        let at = |seconds| Status {
            record_elapsed: seconds,
            ..Status::default()
        };
        assert_eq!(at(65).elapsed_label(), "1:05");
        assert_eq!(at(3725).elapsed_label(), "1:02:05");
    }

    #[test]
    fn a_negative_elapsed_time_does_not_print_a_minus() {
        let status = Status {
            record_elapsed: -5,
            ..Status::default()
        };
        assert_eq!(status.elapsed_label(), "0:00");
    }

    #[test]
    fn zoom_reads_the_way_an_operator_says_it() {
        assert_eq!(Status::default().zoom_label(), None);
        let at = |hundredths| Status {
            zoom_hundredths: Some(hundredths),
            ..Status::default()
        };
        assert_eq!(at(100).zoom_label().as_deref(), Some("1x"));
        assert_eq!(at(250).zoom_label().as_deref(), Some("2.5x"));
        assert_eq!(at(1200).zoom_label().as_deref(), Some("12x"));
    }

    #[test]
    fn shutter_reads_as_a_fraction() {
        assert_eq!(Status::default().shutter_label(), None);
        let status = Status {
            shutter_denominator: Some(50),
            ..Status::default()
        };
        assert_eq!(status.shutter_label().as_deref(), Some("1/50"));
    }

    #[test]
    fn unknown_and_zero_stay_apart() {
        // A camera reporting ISO 0 is not a camera that has said nothing.
        assert_eq!(optional(-1), None);
        assert_eq!(optional(0), Some(0));
        assert_eq!(optional_byte(-1), None);
        assert_eq!(optional_byte(0), Some(0));
    }

    #[test]
    fn a_list_never_reads_past_what_the_camera_reported() {
        let mut values = [0i32; OPC_STATUS_LIST_CAP];
        values[0] = 50;
        values[1] = 60;
        assert_eq!(list(&values, 2), vec![50, 60]);
        assert!(list(&values, 0).is_empty());
        // A count beyond the array is clamped rather than read out of bounds.
        assert_eq!(list(&values, 999).len(), OPC_STATUS_LIST_CAP);
        assert!(list(&values, -1).is_empty());
    }
}
