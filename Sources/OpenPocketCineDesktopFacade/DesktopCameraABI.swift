import COpcDesktop
import Foundation
import OpenPocketViewCore

// The camera link for the desktop shell.
//
// Every byte on the wire is built by `Commands` and `DumlTransport`. The shell owns the
// socket, the clock, and the 40 Hz timer; it never assembles a payload, picks an opcode,
// or decides what an ACK should carry. That split is what keeps a PC watching the same
// camera the phones do without a second reverse-engineering effort.

/// Argument lists arrive positionally. A missing argument is zero, so a builder that
/// takes none can be called with none.
private struct Arguments {
    let ints: [Int]
    let reals: [Double]

    init(
        _ ints: UnsafePointer<Int32>?, _ intCount: Int, _ reals: UnsafePointer<Double>?,
        _ realCount: Int
    ) {
        self.ints = (0..<max(0, intCount)).map { Int(ints?[$0] ?? 0) }
        self.reals = (0..<max(0, realCount)).map { reals?[$0] ?? 0 }
    }

    func int(_ index: Int) -> Int { index < ints.count ? ints[index] : 0 }
    func byte(_ index: Int) -> UInt8 { UInt8(clamping: int(index)) }
    func real(_ index: Int) -> Double { index < reals.count ? reals[index] : 0 }
    func float(_ index: Int) -> Float { Float(real(index)) }
}

/// Builds one command as an encoded DUML frame.
///
/// Returns the byte count needed, writing only when `capacity` allows, or a negative
/// status when the kind or an argument is not one the core accepts.
@_cdecl("opc_camera_command")
func opc_camera_command(
    _ kind: Int32, _ seq: UInt16, _ ints: UnsafePointer<Int32>?, _ intCount: Int,
    _ reals: UnsafePointer<Double>?, _ realCount: Int, _ out: UnsafeMutablePointer<UInt8>?,
    _ capacity: Int
) -> Int64 {
    let arguments = Arguments(ints, intCount, reals, realCount)
    guard let frame = cameraFrame(kind: kind, seq: seq, arguments: arguments) else {
        return Int64(OPC_RELAY_ERR_OUT_OF_RANGE)
    }
    return DesktopFacade.emit(Data(Duml.encode(frame)), into: out, capacity: capacity)
}

/// The opcode key (`set << 8 | cmd`) of the frame `opc_camera_command` would build,
/// so the shell can match a reply to its SET without reading DUML. -1 when the core
/// cannot build that kind.
@_cdecl("opc_camera_command_key")
func opc_camera_command_key(
    _ kind: Int32, _ ints: UnsafePointer<Int32>?, _ intCount: Int,
    _ reals: UnsafePointer<Double>?, _ realCount: Int
) -> Int32 {
    let arguments = Arguments(ints, intCount, reals, realCount)
    guard let frame = cameraFrame(kind: kind, seq: 0, arguments: arguments) else { return -1 }
    return Int32(Duml.opcodeKey(set: frame.cmdSet, cmd: frame.cmdId))
}

// swift-format-ignore: NeverForceUnwrap
private func cameraFrame(kind: Int32, seq: UInt16, arguments: Arguments) -> Duml.Frame? {
    switch kind {
    case OPC_CAM_SESSION_WAKE:
        return Commands.sessionWake()
    case OPC_CAM_SESSION_KEEPALIVE:
        return Commands.sessionKeepalive()
    case OPC_CAM_GIMBAL_INIT:
        return Commands.gimbalInit(seq: seq)
    case OPC_CAM_APP_PRESENCE:
        return Commands.appPresenceFrame(seq: seq)
    case OPC_CAM_LIVE_VIEW_ENABLE:
        return Commands.liveViewEnable(seq: seq)
    case OPC_CAM_NANO_LIVE_GATE:
        return Commands.nanoLiveViewGate(start: arguments.int(0) != 0, seq: seq)

    case OPC_CAM_RECORD_START:
        return Commands.recordStart(seq: seq)
    case OPC_CAM_RECORD_STOP:
        return Commands.recordStop(seq: seq)
    case OPC_CAM_SHOOT_PHOTO:
        return Commands.shootPhoto(seq: seq)
    case OPC_CAM_SET_SHOOTING_MODE:
        return Commands.setShootingMode(raw: arguments.byte(0), seq: seq)

    case OPC_CAM_ZOOM_FACTOR:
        return Commands.setZoom(factor: arguments.real(0), seq: seq)
    case OPC_CAM_ZOOM_LENS:
        return Commands.setZoomLens(UInt16(clamping: arguments.int(0)), seq: seq)
    case OPC_CAM_ZOOM_SLEW:
        return Commands.setZoomSlew(UInt16(clamping: arguments.int(0)), seq: seq)
    case OPC_CAM_ZOOM_STOP:
        return Commands.setZoomStop(seq: seq)

    case OPC_CAM_GIMBAL_RECENTER:
        return Commands.gimbalRecenter(seq: seq)
    case OPC_CAM_GIMBAL_FLIP:
        return Commands.gimbalFlip(seq: seq)
    case OPC_CAM_GIMBAL_FOLLOW:
        return Commands.gimbalFollowFamily(seq: seq)
    case OPC_CAM_GIMBAL_FPV:
        return Commands.gimbalFpv(seq: seq)
    case OPC_CAM_GIMBAL_STICK:
        return Commands.gimbalStick(
            axis0: UInt16(clamping: arguments.int(0)),
            axis1: UInt16(clamping: arguments.int(1)), seq: seq)
    case OPC_CAM_GIMBAL_SPEED:
        guard let speed = GimbalSpeed(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setGimbalSpeed(speed, seq: seq)
    case OPC_CAM_GIMBAL_TIMED_STOP:
        return Commands.gimbalTimedStop(seq: seq)
    case OPC_CAM_GIMBAL_PARAMS_GET:
        return Commands.gimbalParamsGet(seq: seq)
    case OPC_CAM_GIMBAL_TILT_LOCK:
        guard let lock = GimbalTiltLock(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setGimbalTiltLock(lock, seq: seq)

    case OPC_CAM_TRACK_SET:
        return Commands.setTrackingBox(
            id: UInt16(clamping: arguments.int(0)), x: arguments.float(0), y: arguments.float(1),
            width: arguments.float(2), height: arguments.float(3), seq: seq)
    case OPC_CAM_TRACK_CLEAR:
        return Commands.clearTrackingBox(seq: seq)
    case OPC_CAM_TRACK_POLL:
        return Commands.pollTracking(seq: seq)
    case OPC_CAM_FOCUS_TRACK_SET:
        guard let mode = FocusTrackMode(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setFocusTrack(mode, seq: seq)
    case OPC_CAM_FOCUS_TRACK_GET:
        return Commands.getFocusTrack(seq: seq)

    case OPC_CAM_SET_ISO_INDEX:
        guard let index = IsoIndex(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setIsoIndex(index, seq: seq)
    case OPC_CAM_SET_ISO_LIMIT:
        guard let limit = IsoLimit(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setIsoLimit(limit, seq: seq)
    case OPC_CAM_SET_SHUTTER:
        return Commands.setShutter(denom: arguments.int(0), seq: seq)
    case OPC_CAM_SET_EV:
        // Third-stops from zero, clamped by the core: -9 is -3.0 EV, +9 is +3.0.
        return Commands.setEv(EvComp(thirds: arguments.int(0)), seq: seq)
    case OPC_CAM_SET_WB_AUTO:
        return Commands.setWhiteBalanceAuto(tint: arguments.int(0), seq: seq)
    case OPC_CAM_SET_WB_CUSTOM:
        return Commands.setWhiteBalanceCustom(
            kelvin: arguments.int(0), tint: arguments.int(1), seq: seq)
    case OPC_CAM_SET_COLOR_MODE:
        guard let mode = ColorMode(rawValue: arguments.byte(0)) else { return nil }
        let model = CameraModel.resolve(modelId: arguments.int(1), name: nil)
        return Commands.setColorMode(mode, model: model, seq: seq)
    case OPC_CAM_SET_FOCUS_MODE:
        guard let mode = FocusMode(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setFocusMode(mode, seq: seq)
    case OPC_CAM_SET_VIDEO_FORMAT:
        return Commands.setVideoFormat(
            resolution: VideoResolution(rawValue: arguments.byte(0)),
            frameRate: VideoFrameRate(rawValue: arguments.byte(1)), seq: seq)
    case OPC_CAM_SET_FOV:
        guard let fov = FovSetting(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setFov(fov, seq: seq)

    case OPC_CAM_PARAM_GET:
        guard let param = CameraParam(rawValue: UInt16(clamping: arguments.int(0))) else {
            return nil
        }
        return Commands.paramGet(param, seq: seq)
    case OPC_CAM_GET_WIFI_SSID:
        return Commands.getWifiSsid()
    case OPC_CAM_GET_WIFI_PASSWORD:
        return Commands.getWifiPassword()
    case OPC_CAM_ENTER_PLAYBACK:
        return Commands.enterPlayback(seq: seq)
    case OPC_CAM_EXIT_PLAYBACK:
        return Commands.exitPlayback(seq: seq)

    case OPC_CAM_SET_EXPO_MODE:
        guard let mode = ExpoMode(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setExpoMode(mode, seq: seq)
    case OPC_CAM_SET_AUDIO_CHANNEL:
        guard let channel = AudioChannel(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setAudioChannel(channel, seq: seq)
    case OPC_CAM_SET_VOCAL_BOOST:
        guard let boost = VocalBoost(rawValue: arguments.byte(0)) else { return nil }
        return Commands.setVocalBoost(boost, seq: seq)

    case OPC_CAM_MEDIA_LIST:
        return Commands.mediaList(
            counter: arguments.byte(0), cursor: UInt32(truncatingIfNeeded: arguments.int(1)),
            seq: seq)
    case OPC_CAM_MEDIA_LIST_TRIGGER:
        return Commands.mediaListTrigger(seq: seq)
    case OPC_CAM_MEDIA_DELETE:
        return Commands.deleteMedia(
            handle: UInt32(truncatingIfNeeded: arguments.int(0)),
            counter: UInt32(truncatingIfNeeded: arguments.int(1)), seq: seq)
    case OPC_CAM_MEDIA_FAVORITE:
        return Commands.setMediaFavorite(
            handle: UInt32(truncatingIfNeeded: arguments.int(0)), on: arguments.int(2) != 0,
            counter: UInt32(truncatingIfNeeded: arguments.int(1)), seq: seq)
    case OPC_CAM_PLAYBACK_SPECIAL:
        return Commands.pocket3PlaybackEntry(step: arguments.int(0), seq: seq)
    case OPC_CAM_GIMBAL_TIMED_TARGET:
        // Tenths on the wire in, degrees and seconds to the builder, which holds the
        // reach and duration rules and answers nil for anything outside them.
        return Commands.gimbalTimedTarget(
            yawDeg: Double(arguments.int(0)) / 10, nativePitchDeg: Double(arguments.int(1)) / 10,
            duration: Double(arguments.int(2)) / 10, seq: seq)

    case OPC_CAM_TAP_FOCUS_PREPARE:
        return Commands.tapFocusPrepare(seq: seq)
    case OPC_CAM_TAP_FOCUS_POINT:
        return Commands.tapFocusPoint(arguments.float(0), arguments.float(1), seq: seq)
    case OPC_CAM_TAP_FOCUS_HINT:
        return Commands.tapFocusLiveHint(seq: seq)
    case OPC_CAM_TAP_FOCUS_COMMIT:
        return Commands.tapFocusCommit(arguments.float(0), arguments.float(1), seq: seq)

    default:
        return nil
    }
}

/// Tap-to-focus is three frames in order. They are returned as one
/// `[u16le count]([u16le length][frame])…` blob so the shell sends them unchanged.
@_cdecl("opc_camera_tap_focus")
func opc_camera_tap_focus(
    _ x: Float, _ y: Float, _ seq: UInt16, _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    let frames = Commands.tapFocus(x, y, seq: seq)
    var blob: [UInt8] = [UInt8(frames.count & 0xFF), UInt8((frames.count >> 8) & 0xFF)]
    for frame in frames {
        let encoded = Duml.encode(frame)
        blob.append(UInt8(encoded.count & 0xFF))
        blob.append(UInt8((encoded.count >> 8) & 0xFF))
        blob.append(contentsOf: encoded)
    }
    return DesktopFacade.emit(Data(blob), into: out, capacity: capacity)
}

/// Status subscription. `key` is the camera's own subscription name.
@_cdecl("opc_camera_subscribe")
func opc_camera_subscribe(
    _ key: UnsafePointer<CChar>?, _ subId: UInt32, _ seq: UInt16,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let key else { return Int64(OPC_RELAY_ERR_NULL) }
    let frame = Commands.subscribe(key: String(cString: key), subId: subId, seq: seq)
    return DesktopFacade.emit(Data(Duml.encode(frame)), into: out, capacity: capacity)
}
