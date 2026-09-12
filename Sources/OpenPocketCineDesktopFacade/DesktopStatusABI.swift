import COpcDesktop
import Foundation
import OpenPocketViewCore

// Telemetry to a HUD.
//
// The camera describes itself in a stream of status pushes, and `CameraStatusDecoder`
// already knows how to read every one of them — including which body encodes a colour
// mode which way. The desktop shell reads the result; it parses nothing.

private final class StatusBox {
    var status = CameraStatus()
    var model: CameraModel?
}

private func statusBox(_ handle: UnsafeMutableRawPointer?) -> StatusBox? {
    guard let handle else { return nil }
    return Unmanaged<StatusBox>.fromOpaque(handle).takeUnretainedValue()
}

/// `modelId` under zero means the shell has not identified the body yet; the decoder
/// falls back to the encodings every Osmo shares.
@_cdecl("opc_status_create")
func opc_status_create(_ modelId: Int32) -> UnsafeMutableRawPointer {
    let box = StatusBox()
    if modelId >= 0 {
        box.model = CameraModel.resolve(modelId: Int(modelId), name: nil)
    }
    return Unmanaged.passRetained(box).toOpaque()
}

@_cdecl("opc_status_destroy")
func opc_status_destroy(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<StatusBox>.fromOpaque(handle).release()
}

/// Applies one status frame. Non-zero when the decoder recognised it.
@_cdecl("opc_status_apply_frame")
func opc_status_apply_frame(
    _ handle: UnsafeMutableRawPointer?, _ sender: UInt8, _ receiver: UInt8, _ seq: UInt16,
    _ flags: UInt8, _ cmdSet: UInt8, _ cmdId: UInt8, _ payload: UnsafePointer<UInt8>?,
    _ count: Int
) -> Int32 {
    guard let box = statusBox(handle), count >= 0 else { return OPC_RELAY_ERR_NULL }
    let body: [UInt8] =
        payload.map { Array(DesktopFacade.borrow($0, count)) } ?? []
    let frame = Duml.Frame(
        sender: sender, receiver: receiver, seq: seq, flags: flags, cmdSet: cmdSet,
        cmdId: cmdId, payload: body)
    return CameraStatusDecoder.apply(frame, to: &box.status, model: box.model) ? 1 : 0
}

/// Applies a `0x00/0x99` subscription push — where timecode and the camera's own
/// available-value lists arrive.
@_cdecl("opc_status_apply_push")
func opc_status_apply_push(
    _ handle: UnsafeMutableRawPointer?, _ payload: UnsafePointer<UInt8>?, _ count: Int
) -> Int32 {
    guard let box = statusBox(handle), let payload, count >= 0 else {
        return OPC_RELAY_ERR_NULL
    }
    let body = Array(DesktopFacade.borrow(payload, count))
    return CameraStatusDecoder.applySubscribePush(body, to: &box.status, model: box.model)
        ? 1 : 0
}

@_cdecl("opc_status_read")
func opc_status_read(
    _ handle: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<OpcCameraStatus>?
) -> Int32 {
    guard let box = statusBox(handle), let out else { return OPC_RELAY_ERR_NULL }
    let status = box.status

    out.pointee.battery_percent = Int32(clamping: status.batteryPercent)
    out.pointee.charging = status.charging ? 1 : 0
    out.pointee.docked = status.docked ? 1 : 0
    out.pointee.is_recording = status.isRecording ? 1 : 0
    out.pointee.in_playback = status.inPlayback ? 1 : 0
    out.pointee.record_elapsed_sec = Int32(clamping: status.recordElapsedSec)
    out.pointee.record_remaining_sec = Int32(clamping: status.recordRemainingSec)
    out.pointee.shooting_mode = Int32(clamping: status.shootingMode)
    out.pointee.iso = Int32(clamping: status.iso)
    out.pointee.iso_index = status.isoIndex.map { Int32($0.rawValue) } ?? -1
    out.pointee.iso_limit = status.isoLimit.map { Int32($0.rawValue) } ?? -1
    out.pointee.ev_thirds = Int32(clamping: status.evComp?.thirds ?? 0)
    out.pointee.has_ev = status.evComp == nil ? 0 : 1
    out.pointee.shutter_denom = Int32(clamping: status.shutterDenom)
    out.pointee.fps = Int32(clamping: status.fps)
    out.pointee.video_resolution =
        status.videoFormat.map { Int32($0.resolution.rawValue) }
        ?? status.videoResolution.map { Int32($0.rawValue) } ?? -1
    out.pointee.video_frame_rate = status.videoFormat.map { Int32($0.frameRate.rawValue) } ?? -1
    out.pointee.color_mode = status.colorMode.map { Int32($0.rawValue) } ?? -1
    out.pointee.expo_mode = status.expoMode.map { Int32($0.rawValue) } ?? -1
    out.pointee.white_balance_kelvin = Int32(clamping: status.whiteBalanceKelvin)
    out.pointee.white_balance_tint = Int32(clamping: status.whiteBalanceTint ?? 0)
    out.pointee.has_white_balance_tint = status.whiteBalanceTint == nil ? 0 : 1
    out.pointee.focus_mode = status.focusMode.map { Int32($0.rawValue) } ?? -1
    out.pointee.focus_track = status.focusTrack.map { Int32($0.rawValue) } ?? -1
    out.pointee.storage_free_mb = Int32(clamping: status.storageFreeMb)
    out.pointee.storage_total_mb = Int32(clamping: status.storageTotalMb)
    out.pointee.zoom_hundredths =
        status.zoomFactor.map { Int32(($0 * 100).rounded()) } ?? -1
    out.pointee.reserved = 0

    let cap = Int(OPC_STATUS_LIST_CAP)
    let shutters = withUnsafeMutablePointer(to: &out.pointee.available_shutter) {
        DesktopFacade.writeInts(
            status.availableShutterDenoms, into: UnsafeMutableRawPointer($0), capacity: cap)
    }
    let isos = withUnsafeMutablePointer(to: &out.pointee.available_iso) {
        DesktopFacade.writeInts(
            status.availableIsoIndices.map { Int($0.rawValue) },
            into: UnsafeMutableRawPointer($0), capacity: cap)
    }
    let resolutions = withUnsafeMutablePointer(to: &out.pointee.available_format_resolution) {
        DesktopFacade.writeInts(
            status.availableVideoFormats.map { Int($0.resolution.rawValue) },
            into: UnsafeMutableRawPointer($0), capacity: cap)
    }
    withUnsafeMutablePointer(to: &out.pointee.available_format_frame_rate) {
        _ = DesktopFacade.writeInts(
            status.availableVideoFormats.map { Int($0.frameRate.rawValue) },
            into: UnsafeMutableRawPointer($0), capacity: cap)
    }
    let colors = withUnsafeMutablePointer(to: &out.pointee.available_color) {
        DesktopFacade.writeInts(
            status.availableColorModes.map { Int($0.rawValue) },
            into: UnsafeMutableRawPointer($0), capacity: cap)
    }
    out.pointee.available_shutter_count = shutters
    out.pointee.available_iso_count = isos
    out.pointee.available_format_count = resolutions
    out.pointee.available_color_count = colors
    return OPC_RELAY_OK
}

/// The camera's timecode, when it is pushing one.
@_cdecl("opc_status_timecode")
func opc_status_timecode(
    _ handle: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let box = statusBox(handle) else { return Int64(OPC_RELAY_ERR_NULL) }
    return DesktopFacade.emit(
        Data((box.status.timecode ?? "").utf8), into: out, capacity: capacity)
}

@_cdecl("opc_status_firmware")
func opc_status_firmware(
    _ handle: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let box = statusBox(handle) else { return Int64(OPC_RELAY_ERR_NULL) }
    return DesktopFacade.emit(
        Data((box.status.firmware ?? "").utf8), into: out, capacity: capacity)
}

/// The subscription key a shell asks for to start receiving pushes.
@_cdecl("opc_status_subscribe_keys")
func opc_status_subscribe_keys(_ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int) -> Int64 {
    let keys = [CamCapShutter.subscribeKey, "timecode_info"].joined(separator: "\n")
    return DesktopFacade.emit(Data(keys.utf8), into: out, capacity: capacity)
}
