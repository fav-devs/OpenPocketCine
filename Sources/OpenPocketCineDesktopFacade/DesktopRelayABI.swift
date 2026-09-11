import COpcDesktop
import Foundation
import OpenPocketViewCore

// Wire reading and writing for the desktop watcher. Offsets are returned instead of
// copies so the host can decode straight out of its own receive buffer.

@_cdecl("opc_relay_framing_decode")
func opc_relay_framing_decode(
    _ buffer: UnsafePointer<UInt8>?, _ count: Int,
    _ out: UnsafeMutablePointer<OpcRelayMessageHeader>?
) -> Int32 {
    guard let buffer, let out, count >= 0 else { return OPC_RELAY_ERR_NULL }
    let data = DesktopFacade.borrow(buffer, count)
    do {
        guard let decoded = try WatcherRelayFraming.decode(from: data) else {
            return OPC_RELAY_NEED_MORE
        }
        out.pointee.kind = decoded.kind.rawValue
        out.pointee.reserved = (0, 0, 0)
        out.pointee.payload_len = UInt32(decoded.payload.count)
        out.pointee.payload_offset = UInt32(decoded.consumedBytes - decoded.payload.count)
        out.pointee.consumed = UInt32(decoded.consumedBytes)
        return OPC_RELAY_OK
    } catch WatcherRelayFraming.DecodeError.unknownKind(_) {
        return OPC_RELAY_ERR_UNKNOWN_KIND
    } catch WatcherRelayFraming.DecodeError.payloadTooLarge(_) {
        return OPC_RELAY_ERR_PAYLOAD_TOO_LARGE
    } catch {
        return OPC_RELAY_ERR_MALFORMED
    }
}

@_cdecl("opc_relay_framing_encode")
func opc_relay_framing_encode(
    _ kind: UInt8, _ payload: UnsafePointer<UInt8>?, _ payloadCount: Int,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let messageKind = WatcherRelayProtocol.Kind(rawValue: kind) else {
        return Int64(OPC_RELAY_ERR_UNKNOWN_KIND)
    }
    guard payloadCount >= 0 else { return Int64(OPC_RELAY_ERR_NULL) }
    let body: Data
    if let payload, payloadCount > 0 {
        body = DesktopFacade.borrow(payload, payloadCount)
    } else {
        body = Data()
    }
    let wire = WatcherRelayFraming.encode(kind: messageKind, payload: body)
    return DesktopFacade.emit(wire, into: out, capacity: capacity)
}

@_cdecl("opc_relay_frame_blob_decode")
func opc_relay_frame_blob_decode(
    _ payload: UnsafePointer<UInt8>?, _ count: Int,
    _ split: UnsafeMutablePointer<OpcRelayBlobSplit>?,
    _ meta: UnsafeMutablePointer<OpcRelayFrameMeta>?
) -> Int32 {
    guard let payload, let split, let meta, count >= 0 else { return OPC_RELAY_ERR_NULL }
    let data = DesktopFacade.borrow(payload, count)
    guard let (metadata, hevc) = try? WatcherRelayFrameBlob.decode(data) else {
        return OPC_RELAY_ERR_MALFORMED
    }
    let hevcOffset = count - hevc.count
    guard hevcOffset >= 4 else { return OPC_RELAY_ERR_MALFORMED }
    split.pointee.meta_offset = 4
    split.pointee.meta_len = UInt32(hevcOffset - 4)
    split.pointee.hevc_offset = UInt32(hevcOffset)
    split.pointee.hevc_len = UInt32(hevc.count)
    meta.pointee.codec = Int32(metadata.codec)
    meta.pointee.is_keyframe = metadata.isKeyframe ? 1 : 0
    meta.pointee.is_recording = metadata.isRecording ? 1 : 0
    meta.pointee.extra_mirrored = metadata.extraMirrored ? 1 : 0
    meta.pointee.has_encoded_at = metadata.encodedAt == nil ? 0 : 1
    meta.pointee.encoded_at = metadata.encodedAt ?? 0
    meta.pointee.parameter_set_count = Int32(metadata.parameterSets?.count ?? 0)
    return OPC_RELAY_OK
}

/// Copies one VPS/SPS/PPS blob out of a keyframe's metadata JSON.
@_cdecl("opc_relay_frame_parameter_set")
func opc_relay_frame_parameter_set(
    _ metaJSON: UnsafePointer<UInt8>?, _ count: Int, _ index: Int32,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let metaJSON, count >= 0, index >= 0 else { return Int64(OPC_RELAY_ERR_NULL) }
    let data = DesktopFacade.borrow(metaJSON, count)
    guard
        let metadata = try? JSONDecoder().decode(WatcherRelayFrameMetadata.self, from: data),
        let sets = metadata.parameterSets, Int(index) < sets.count
    else { return Int64(OPC_RELAY_ERR_OUT_OF_RANGE) }
    return DesktopFacade.emit(sets[Int(index)], into: out, capacity: capacity)
}

@_cdecl("opc_relay_state_decode")
func opc_relay_state_decode(
    _ json: UnsafePointer<UInt8>?, _ count: Int, _ out: UnsafeMutablePointer<OpcRelayState>?
) -> Int32 {
    guard let json, let out, count >= 0 else { return OPC_RELAY_ERR_NULL }
    let data = DesktopFacade.borrow(json, count)
    guard let state = try? JSONDecoder().decode(WatcherRelayState.self, from: data) else {
        return OPC_RELAY_ERR_MALFORMED
    }
    out.pointee.is_recording = state.isRecording ? 1 : 0
    out.pointee.battery_percent = Int32(clamping: state.batteryPercent)
    out.pointee.allows_control_requests = state.allowsControlRequests ? 1 : 0
    out.pointee.is_nano = state.isNano.map { $0 ? Int32(1) : Int32(0) } ?? -1
    let options = state.controlOptions
    out.pointee.has_control_options = options == nil ? 0 : 1
    let cap = DesktopFacade.optionsCapacity
    // Counts land after the array writes so no two accesses to `out.pointee` overlap.
    let isoCount = withUnsafeMutablePointer(to: &out.pointee.iso_indices) {
        DesktopFacade.writeInts(
            options?.isoIndices ?? [], into: UnsafeMutableRawPointer($0), capacity: cap)
    }
    let shutterCount = withUnsafeMutablePointer(to: &out.pointee.shutter_denominators) {
        DesktopFacade.writeInts(
            options?.shutterDenominators ?? [], into: UnsafeMutableRawPointer($0), capacity: cap)
    }
    let zoomCount = withUnsafeMutablePointer(to: &out.pointee.zoom_hundredths) {
        DesktopFacade.writeInts(
            options?.zoomHundredths ?? [], into: UnsafeMutableRawPointer($0), capacity: cap)
    }
    out.pointee.iso_count = isoCount
    out.pointee.shutter_count = shutterCount
    out.pointee.zoom_count = zoomCount
    let text = DesktopFacade.textCapacity
    withUnsafeMutablePointer(to: &out.pointee.format) {
        DesktopFacade.writeText(state.format, into: UnsafeMutableRawPointer($0), capacity: text)
    }
    withUnsafeMutablePointer(to: &out.pointee.color) {
        DesktopFacade.writeText(state.color, into: UnsafeMutableRawPointer($0), capacity: text)
    }
    withUnsafeMutablePointer(to: &out.pointee.zoom) {
        DesktopFacade.writeText(state.zoom, into: UnsafeMutableRawPointer($0), capacity: text)
    }
    withUnsafeMutablePointer(to: &out.pointee.live_fps) {
        DesktopFacade.writeText(state.liveFPS, into: UnsafeMutableRawPointer($0), capacity: text)
    }
    withUnsafeMutablePointer(to: &out.pointee.camera_name) {
        DesktopFacade.writeText(state.cameraName, into: UnsafeMutableRawPointer($0), capacity: text)
    }
    withUnsafeMutablePointer(to: &out.pointee.iso) {
        DesktopFacade.writeText(state.iso, into: UnsafeMutableRawPointer($0), capacity: text)
    }
    withUnsafeMutablePointer(to: &out.pointee.shutter) {
        DesktopFacade.writeText(state.shutter, into: UnsafeMutableRawPointer($0), capacity: text)
    }
    withUnsafeMutablePointer(to: &out.pointee.camera_model) {
        DesktopFacade.writeText(
            state.cameraModel ?? "", into: UnsafeMutableRawPointer($0), capacity: text)
    }
    return OPC_RELAY_OK
}

@_cdecl("opc_relay_join_denied_decode")
func opc_relay_join_denied_decode(
    _ json: UnsafePointer<UInt8>?, _ count: Int, _ out: UnsafeMutablePointer<OpcRelayJoinDenied>?
) -> Int32 {
    guard let json, let out, count >= 0 else { return OPC_RELAY_ERR_NULL }
    let data = DesktopFacade.borrow(json, count)
    guard let denied = try? JSONDecoder().decode(WatcherRelayJoinDenied.self, from: data) else {
        return OPC_RELAY_ERR_MALFORMED
    }
    out.pointee.passcode_required = denied.passcodeRequired ? 1 : 0
    withUnsafeMutablePointer(to: &out.pointee.reason) {
        DesktopFacade.writeText(
            denied.reason, into: UnsafeMutableRawPointer($0),
            capacity: DesktopFacade.reasonCapacity)
    }
    return OPC_RELAY_OK
}

@_cdecl("opc_relay_control_token_decode")
func opc_relay_control_token_decode(
    _ json: UnsafePointer<UInt8>?, _ count: Int, _ out: UnsafeMutablePointer<OpcRelayControlToken>?
) -> Int32 {
    guard let json, let out, count >= 0 else { return OPC_RELAY_ERR_NULL }
    let data = DesktopFacade.borrow(json, count)
    guard let token = try? JSONDecoder().decode(WatcherRelayControlToken.self, from: data) else {
        return OPC_RELAY_ERR_MALFORMED
    }
    out.pointee.holder_is_recipient = token.holderIsRecipient ? 1 : 0
    withUnsafeMutablePointer(to: &out.pointee.holder_name) {
        DesktopFacade.writeText(
            token.holderName, into: UnsafeMutableRawPointer($0),
            capacity: DesktopFacade.textCapacity)
    }
    return OPC_RELAY_OK
}

@_cdecl("opc_relay_hello_decode")
func opc_relay_hello_decode(
    _ json: UnsafePointer<UInt8>?, _ count: Int, _ out: UnsafeMutablePointer<OpcRelayHello>?
) -> Int32 {
    guard let json, let out, count >= 0 else { return OPC_RELAY_ERR_NULL }
    let data = DesktopFacade.borrow(json, count)
    guard let hello = try? JSONDecoder().decode(WatcherRelayHello.self, from: data) else {
        return OPC_RELAY_ERR_MALFORMED
    }
    out.pointee.version = Int32(hello.version)
    withUnsafeMutablePointer(to: &out.pointee.host_name) {
        DesktopFacade.writeText(
            hello.hostName, into: UnsafeMutableRawPointer($0),
            capacity: DesktopFacade.textCapacity)
    }
    withUnsafeMutablePointer(to: &out.pointee.camera_name) {
        DesktopFacade.writeText(
            hello.cameraName ?? "", into: UnsafeMutableRawPointer($0),
            capacity: DesktopFacade.textCapacity)
    }
    return OPC_RELAY_OK
}

/// Builds the watcher's own hello. Empty `passcode` and `watcherID` are sent as absent.
@_cdecl("opc_relay_hello_encode")
func opc_relay_hello_encode(
    _ hostName: UnsafePointer<CChar>?, _ passcode: UnsafePointer<CChar>?,
    _ watcherID: UnsafePointer<CChar>?, _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let hostName else { return Int64(OPC_RELAY_ERR_NULL) }
    func optional(_ pointer: UnsafePointer<CChar>?) -> String? {
        guard let pointer else { return nil }
        let text = String(cString: pointer)
        return text.isEmpty ? nil : text
    }
    let hello = WatcherRelayHello(
        hostName: String(cString: hostName), passcode: optional(passcode),
        watcherID: optional(watcherID))
    guard let payload = try? JSONEncoder().encode(hello) else {
        return Int64(OPC_RELAY_ERR_MALFORMED)
    }
    return DesktopFacade.emit(payload, into: out, capacity: capacity)
}

/// Encodes a `WatcherRelayCommand`. Unused argument slots are ignored per case.
@_cdecl("opc_relay_command_encode")
func opc_relay_command_encode(
    _ kind: Int32, _ a: Int32, _ b: Int32, _ c: Int32, _ d: Int32,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    let command: WatcherRelayCommand
    switch kind {
    case OPC_RELAY_COMMAND_TOGGLE_RECORDING:
        command = .toggleRecording
    case OPC_RELAY_COMMAND_TAP_FOCUS:
        command = .tapFocus(
            cameraX: Int(a), cameraY: Int(b), coordinateWidth: Int(c), coordinateHeight: Int(d))
    case OPC_RELAY_COMMAND_SET_ISO:
        command = .setISO(Int(a))
    case OPC_RELAY_COMMAND_SET_SHUTTER_DENOM:
        command = .setShutterDenom(Int(a))
    case OPC_RELAY_COMMAND_SET_WHITE_BALANCE:
        command = .setWhiteBalance(mode: Int(a), kelvin: Int(b), tint: Int(c))
    case OPC_RELAY_COMMAND_SET_COLOR:
        command = .setColor(Int(a))
    case OPC_RELAY_COMMAND_SET_ZOOM:
        command = .setZoom(Int(a))
    default:
        return Int64(OPC_RELAY_ERR_OUT_OF_RANGE)
    }
    guard let payload = try? JSONEncoder().encode(command) else {
        return Int64(OPC_RELAY_ERR_MALFORMED)
    }
    return DesktopFacade.emit(payload, into: out, capacity: capacity)
}

@_cdecl("opc_relay_focus_map")
func opc_relay_focus_map(
    _ x: Double, _ y: Double, _ width: Double, _ height: Double, _ mirrored: Int32,
    _ out: UnsafeMutablePointer<OpcRelayFocusPoint>?
) -> Int32 {
    guard let out else { return OPC_RELAY_ERR_NULL }
    guard
        let point = WatcherFocusPoint.map(
            x: x, y: y, width: width, height: height, mirrored: mirrored != 0)
    else { return OPC_RELAY_ERR_OUT_OF_RANGE }
    out.pointee.x = Int32(clamping: point.x)
    out.pointee.y = Int32(clamping: point.y)
    return OPC_RELAY_OK
}
