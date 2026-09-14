//! The commands a v1 desktop operator can send.
//!
//! Each case maps to exactly one builder in the core's `Commands`. The arguments are
//! carried positionally across the C boundary, which is why this file and
//! `DesktopCameraABI.swift` have to agree case by case — the round-trip tests in
//! `tests/` are what hold them together.

use opc_core_sys as sys;

use crate::CameraError;

/// A camera write or read. Values are the core's own raw encodings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    // Session.
    SessionWake,
    SessionKeepalive,
    GimbalInit,
    AppPresence,
    /// `0x09/0xa8`. Enable-once: the watchdog owns every repeat.
    LiveViewEnable,
    NanoLiveGate {
        start: bool,
    },

    // Capture.
    RecordStart,
    RecordStop,
    ShootPhoto,
    SetShootingMode(u8),

    // Zoom.
    /// A slider tick: coalesced, never retransmitted, pipelined at 20 Hz.
    ZoomFactor(f64),
    /// A chip stop or a key: urgent, retransmitted, announced.
    ZoomJump(f64),
    ZoomLens(u16),
    /// Continuous slew; pair with `ZoomStop`.
    ZoomSlew(u16),
    ZoomStop,

    // Gimbal.
    GimbalRecenter,
    GimbalFlip,
    GimbalFollow,
    GimbalFpv,
    /// Notify, not a round trip. Rides the ACK queue at 25 Hz while held.
    GimbalStick {
        axis0: u16,
        axis1: u16,
    },
    GimbalSpeed(u8),
    GimbalTimedStop,
    GimbalParamsGet,
    GimbalTiltLock(u8),

    // Tracking.
    TrackSet {
        id: u16,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    TrackClear,
    TrackPoll,
    FocusTrackSet(u8),
    FocusTrackGet,
    /// Mimo's tap-to-focus burst, in order: `0x22` spot, `0x30` region, `0x68` hint,
    /// `0x32` commit. `x` and `y` are picture fractions on the sensor.
    TapFocusPrepare,
    TapFocusPoint {
        x: f32,
        y: f32,
    },
    TapFocusHint,
    TapFocusCommit {
        x: f32,
        y: f32,
    },
    /// `0x02/0xA0`: read the audio DSP blob. Wind and directional need it first.
    AudioDspGet,
    /// `0x02/0x9F`: the body's own blob with `@2` patched for wind noise reduction.
    AudioWind {
        on: bool,
        blob: [u8; crate::status::AUDIO_DSP_BLOB],
    },
    /// `0x02/0x9F`: the blob with `@2` patched for directional audio (`0xDA` all,
    /// `0x3A` front, `0xBA` front+back).
    AudioDirectional {
        mode: u8,
        blob: [u8; crate::status::AUDIO_DSP_BLOB],
    },

    // Exposure and look.
    SetIsoIndex(u8),
    SetIsoLimit(u8),
    /// The N in 1/N, 4 through 16000.
    SetShutter(i32),
    /// Third-stops from zero: -9 is -3.0 EV, +9 is +3.0.
    SetEv(i32),
    SetWhiteBalanceAuto {
        tint: i32,
    },
    SetWhiteBalanceCustom {
        kelvin: i32,
        tint: i32,
    },
    /// `model_id` lets the core pick the body's own encoding for this mode.
    SetColorMode {
        mode: u8,
        model_id: i32,
    },
    SetFocusMode(u8),
    SetVideoFormat {
        resolution: u8,
        frame_rate: u8,
    },
    SetFov(u8),
    /// `0x01` auto, `0x04` manual. No GET; `cam_expo_param` echoes it.
    SetExpoMode(u8),
    /// `0x01` mono, `0x02` stereo, `0x03` spatial.
    SetAudioChannel(u8),
    /// `0x00` off, `0x01` on.
    SetVocalBoost(u8),

    // Media.
    /// `0x00/0x26`. Counter 1 lists the card, 2 the internal store; the cursor is the
    /// newest marker (`0x00000001` / `0x40000001`) or a video handle to page older.
    MediaList {
        counter: u8,
        cursor: u32,
    },
    /// The `4a040e10` trigger sent between the two list queries of a page.
    MediaListTrigger,
    /// `0x00/0x28`. Irreversible; the shell sends a handle only when the fit vouched for it.
    MediaDelete {
        handle: u32,
        counter: u32,
    },
    /// `0x02/0xBF`.
    MediaFavorite {
        handle: u32,
        counter: u32,
        on: bool,
    },
    /// `0x01/0x01` Pocket 3 playback entry, step 1 or 2.
    PlaybackSpecial(u8),
    /// `0x04/0x14` absolute timed target: yaw and native pitch in 0.1°, duration in
    /// tenths of a second. The core refuses one outside the reach or the 0.1–25.5 s
    /// window.
    GimbalTimedTarget {
        yaw_tenth: i32,
        native_pitch_tenth: i32,
        duration_tenths: u8,
    },

    // Reads.
    ParamGet(u16),
    GetWifiSsid,
    GetWifiPassword,
    EnterPlayback,
    ExitPlayback,
}

impl Command {
    /// The kind tag and positional arguments the facade expects.
    fn parts(self) -> (i32, Vec<i32>, Vec<f64>) {
        let ints = |values: &[i32]| values.to_vec();
        match self {
            Self::SessionWake => (sys::OPC_CAM_SESSION_WAKE, vec![], vec![]),
            Self::SessionKeepalive => (sys::OPC_CAM_SESSION_KEEPALIVE, vec![], vec![]),
            Self::GimbalInit => (sys::OPC_CAM_GIMBAL_INIT, vec![], vec![]),
            Self::AppPresence => (sys::OPC_CAM_APP_PRESENCE, vec![], vec![]),
            Self::LiveViewEnable => (sys::OPC_CAM_LIVE_VIEW_ENABLE, vec![], vec![]),
            Self::NanoLiveGate { start } => (
                sys::OPC_CAM_NANO_LIVE_GATE,
                ints(&[i32::from(start)]),
                vec![],
            ),

            Self::RecordStart => (sys::OPC_CAM_RECORD_START, vec![], vec![]),
            Self::RecordStop => (sys::OPC_CAM_RECORD_STOP, vec![], vec![]),
            Self::ShootPhoto => (sys::OPC_CAM_SHOOT_PHOTO, vec![], vec![]),
            Self::SetShootingMode(mode) => (
                sys::OPC_CAM_SET_SHOOTING_MODE,
                ints(&[i32::from(mode)]),
                vec![],
            ),

            Self::ZoomFactor(factor) | Self::ZoomJump(factor) => {
                (sys::OPC_CAM_ZOOM_FACTOR, vec![], vec![factor])
            }
            Self::ZoomLens(position) => {
                (sys::OPC_CAM_ZOOM_LENS, ints(&[i32::from(position)]), vec![])
            }
            Self::ZoomSlew(value) => (sys::OPC_CAM_ZOOM_SLEW, ints(&[i32::from(value)]), vec![]),
            Self::ZoomStop => (sys::OPC_CAM_ZOOM_STOP, vec![], vec![]),

            Self::GimbalRecenter => (sys::OPC_CAM_GIMBAL_RECENTER, vec![], vec![]),
            Self::GimbalFlip => (sys::OPC_CAM_GIMBAL_FLIP, vec![], vec![]),
            Self::GimbalFollow => (sys::OPC_CAM_GIMBAL_FOLLOW, vec![], vec![]),
            Self::GimbalFpv => (sys::OPC_CAM_GIMBAL_FPV, vec![], vec![]),
            Self::GimbalStick { axis0, axis1 } => (
                sys::OPC_CAM_GIMBAL_STICK,
                ints(&[i32::from(axis0), i32::from(axis1)]),
                vec![],
            ),
            Self::GimbalSpeed(speed) => {
                (sys::OPC_CAM_GIMBAL_SPEED, ints(&[i32::from(speed)]), vec![])
            }
            Self::GimbalTimedStop => (sys::OPC_CAM_GIMBAL_TIMED_STOP, vec![], vec![]),
            Self::GimbalParamsGet => (sys::OPC_CAM_GIMBAL_PARAMS_GET, vec![], vec![]),
            Self::GimbalTiltLock(lock) => (
                sys::OPC_CAM_GIMBAL_TILT_LOCK,
                ints(&[i32::from(lock)]),
                vec![],
            ),

            Self::TrackSet {
                id,
                x,
                y,
                width,
                height,
            } => (
                sys::OPC_CAM_TRACK_SET,
                ints(&[i32::from(id)]),
                vec![
                    f64::from(x),
                    f64::from(y),
                    f64::from(width),
                    f64::from(height),
                ],
            ),
            Self::TrackClear => (sys::OPC_CAM_TRACK_CLEAR, vec![], vec![]),
            Self::TrackPoll => (sys::OPC_CAM_TRACK_POLL, vec![], vec![]),
            Self::FocusTrackSet(mode) => (
                sys::OPC_CAM_FOCUS_TRACK_SET,
                ints(&[i32::from(mode)]),
                vec![],
            ),
            Self::FocusTrackGet => (sys::OPC_CAM_FOCUS_TRACK_GET, vec![], vec![]),
            Self::TapFocusPrepare => (sys::OPC_CAM_TAP_FOCUS_PREPARE, vec![], vec![]),
            Self::TapFocusPoint { x, y } => (
                sys::OPC_CAM_TAP_FOCUS_POINT,
                vec![],
                vec![f64::from(x), f64::from(y)],
            ),
            Self::TapFocusHint => (sys::OPC_CAM_TAP_FOCUS_HINT, vec![], vec![]),
            Self::AudioDspGet => (sys::OPC_CAM_AUDIO_DSP_GET, vec![], vec![]),
            Self::AudioWind { on, blob } => (
                sys::OPC_CAM_AUDIO_WIND,
                std::iter::once(i32::from(on))
                    .chain(blob.iter().map(|byte| i32::from(*byte)))
                    .collect(),
                vec![],
            ),
            Self::AudioDirectional { mode, blob } => (
                sys::OPC_CAM_AUDIO_DIRECTIONAL,
                std::iter::once(i32::from(mode))
                    .chain(blob.iter().map(|byte| i32::from(*byte)))
                    .collect(),
                vec![],
            ),
            Self::TapFocusCommit { x, y } => (
                sys::OPC_CAM_TAP_FOCUS_COMMIT,
                vec![],
                vec![f64::from(x), f64::from(y)],
            ),

            Self::SetIsoIndex(index) => (
                sys::OPC_CAM_SET_ISO_INDEX,
                ints(&[i32::from(index)]),
                vec![],
            ),
            Self::SetIsoLimit(limit) => (
                sys::OPC_CAM_SET_ISO_LIMIT,
                ints(&[i32::from(limit)]),
                vec![],
            ),
            Self::SetShutter(denominator) => {
                (sys::OPC_CAM_SET_SHUTTER, ints(&[denominator]), vec![])
            }
            Self::SetEv(thirds) => (sys::OPC_CAM_SET_EV, ints(&[thirds]), vec![]),
            Self::SetWhiteBalanceAuto { tint } => (sys::OPC_CAM_SET_WB_AUTO, ints(&[tint]), vec![]),
            Self::SetWhiteBalanceCustom { kelvin, tint } => {
                (sys::OPC_CAM_SET_WB_CUSTOM, ints(&[kelvin, tint]), vec![])
            }
            Self::SetColorMode { mode, model_id } => (
                sys::OPC_CAM_SET_COLOR_MODE,
                ints(&[i32::from(mode), model_id]),
                vec![],
            ),
            Self::SetFocusMode(mode) => (
                sys::OPC_CAM_SET_FOCUS_MODE,
                ints(&[i32::from(mode)]),
                vec![],
            ),
            Self::SetVideoFormat {
                resolution,
                frame_rate,
            } => (
                sys::OPC_CAM_SET_VIDEO_FORMAT,
                ints(&[i32::from(resolution), i32::from(frame_rate)]),
                vec![],
            ),
            Self::SetFov(fov) => (sys::OPC_CAM_SET_FOV, ints(&[i32::from(fov)]), vec![]),

            Self::ParamGet(pid) => (sys::OPC_CAM_PARAM_GET, ints(&[i32::from(pid)]), vec![]),
            Self::GetWifiSsid => (sys::OPC_CAM_GET_WIFI_SSID, vec![], vec![]),
            Self::GetWifiPassword => (sys::OPC_CAM_GET_WIFI_PASSWORD, vec![], vec![]),
            Self::EnterPlayback => (sys::OPC_CAM_ENTER_PLAYBACK, vec![], vec![]),
            Self::ExitPlayback => (sys::OPC_CAM_EXIT_PLAYBACK, vec![], vec![]),
            Self::SetExpoMode(mode) => {
                (sys::OPC_CAM_SET_EXPO_MODE, ints(&[i32::from(mode)]), vec![])
            }
            Self::SetAudioChannel(channel) => (
                sys::OPC_CAM_SET_AUDIO_CHANNEL,
                ints(&[i32::from(channel)]),
                vec![],
            ),
            Self::SetVocalBoost(boost) => (
                sys::OPC_CAM_SET_VOCAL_BOOST,
                ints(&[i32::from(boost)]),
                vec![],
            ),
            Self::MediaList { counter, cursor } => (
                sys::OPC_CAM_MEDIA_LIST,
                ints(&[i32::from(counter), cursor as i32]),
                vec![],
            ),
            Self::MediaListTrigger => (sys::OPC_CAM_MEDIA_LIST_TRIGGER, vec![], vec![]),
            Self::MediaDelete { handle, counter } => (
                sys::OPC_CAM_MEDIA_DELETE,
                ints(&[handle as i32, counter as i32]),
                vec![],
            ),
            Self::MediaFavorite {
                handle,
                counter,
                on,
            } => (
                sys::OPC_CAM_MEDIA_FAVORITE,
                ints(&[handle as i32, counter as i32, i32::from(on)]),
                vec![],
            ),
            Self::PlaybackSpecial(step) => (
                sys::OPC_CAM_PLAYBACK_SPECIAL,
                ints(&[i32::from(step)]),
                vec![],
            ),
            Self::GimbalTimedTarget {
                yaw_tenth,
                native_pitch_tenth,
                duration_tenths,
            } => (
                sys::OPC_CAM_GIMBAL_TIMED_TARGET,
                ints(&[yaw_tenth, native_pitch_tenth, i32::from(duration_tenths)]),
                vec![],
            ),
        }
    }

    /// Builds this command as an encoded DUML frame, CRC included.
    /// Mimo's tap-to-focus burst for a point on the sensor. AF-S and AF-C are the same
    /// four writes; the phones wait for the region's ACK before the last two.
    pub fn tap_focus(x: f32, y: f32) -> [Self; 4] {
        let x = x.clamp(0.0, 1.0);
        let y = y.clamp(0.0, 1.0);
        [
            Self::TapFocusPrepare,
            Self::TapFocusPoint { x, y },
            Self::TapFocusHint,
            Self::TapFocusCommit { x, y },
        ]
    }

    /// The opcode key (`set << 8 | cmd`) of the frame this becomes, from the core.
    /// `None` without the core, or for a command it cannot build.
    #[cfg(opc_core_linked)]
    pub fn opcode_key(self) -> Option<u16> {
        let (kind, ints, reals) = self.parts();
        // Safety: both argument slices outlive the call.
        let key = unsafe {
            sys::opc_camera_command_key(
                kind,
                ints.as_ptr(),
                ints.len(),
                reals.as_ptr(),
                reals.len(),
            )
        };
        u16::try_from(key).ok()
    }

    #[cfg(not(opc_core_linked))]
    pub fn opcode_key(self) -> Option<u16> {
        None
    }

    /// Whether this write is one the SET mailbox governs, as the core lists them.
    pub fn is_live_control(self) -> bool {
        self.opcode_key().is_some_and(is_live_control)
    }

    /// A slider tick that must not be retransmitted and may coalesce.
    pub fn is_slider(self) -> bool {
        matches!(self, Self::ZoomFactor(_) | Self::ZoomLens(_))
    }

    /// Writes the phones fire without a retransmit: one photo is one photo.
    pub fn retransmits(self) -> bool {
        !matches!(self, Self::ShootPhoto) && !self.is_slider()
    }

    pub fn encode(self, seq: u16) -> Result<Vec<u8>, CameraError> {
        let (kind, ints, reals) = self.parts();
        let call = |out: *mut u8, capacity: usize| {
            // Safety: both argument slices outlive the call, and the core writes at most
            // `capacity` bytes into `out`.
            unsafe {
                sys::opc_camera_command(
                    kind,
                    seq,
                    ints.as_ptr(),
                    ints.len(),
                    reals.as_ptr(),
                    reals.len(),
                    out,
                    capacity,
                )
            }
        };
        let needed = call(std::ptr::null_mut(), 0);
        if needed < 0 {
            return Err(CameraError::Rejected(self));
        }
        let mut out = vec![0u8; needed as usize];
        let written = call(out.as_mut_ptr(), out.len());
        if written < 0 {
            return Err(CameraError::Rejected(self));
        }
        out.truncate(written as usize);
        Ok(out)
    }
}

/// Whether an opcode key is one the SET mailbox governs.
#[cfg(opc_core_linked)]
pub fn is_live_control(key: u16) -> bool {
    // Safety: a plain value in.
    unsafe { sys::opc_duml_is_live_control(i32::from(key)) != 0 }
}

#[cfg(not(opc_core_linked))]
pub fn is_live_control(_key: u16) -> bool {
    false
}

/// The key a reply frame carries, packed the way the core packs it.
#[cfg(opc_core_linked)]
pub fn opcode_key(cmd_set: u8, cmd_id: u8) -> Option<u16> {
    // Safety: plain values in.
    let key = unsafe { sys::opc_duml_opcode_key(i32::from(cmd_set), i32::from(cmd_id)) };
    u16::try_from(key).ok()
}

#[cfg(not(opc_core_linked))]
pub fn opcode_key(_cmd_set: u8, _cmd_id: u8) -> Option<u16> {
    None
}
