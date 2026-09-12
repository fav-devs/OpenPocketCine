import COpcDesktop
import Foundation
import OpenPocketViewCore

/// Shared helpers for the desktop C ABI.
///
/// The desktop shell (`Apps/Desktop/`) owns sockets, discovery, decoding, and drawing.
/// Everything it is not allowed to reinvent — framing limits, kind validation, join
/// rules, recovery deadlines, focus fitting — is reached through these exports so the
/// Windows host and the Apple shells run one implementation.
enum DesktopFacade {
    static let textCapacity = Int(OPC_RELAY_TEXT_CAP)
    static let reasonCapacity = Int(OPC_RELAY_REASON_CAP)
    static let optionsCapacity = Int(OPC_RELAY_OPTIONS_CAP)

    /// UTF-8 bytes for `text` that fit in `capacity` including the terminator, never
    /// splitting a scalar across the limit.
    static func clampedUTF8(_ text: String, capacity: Int) -> [UInt8] {
        guard capacity > 1 else { return [] }
        var out: [UInt8] = []
        for scalar in text.unicodeScalars {
            var encoded: [UInt8] = []
            UTF8.encode(scalar) { encoded.append($0) }
            if out.count + encoded.count > capacity - 1 { break }
            out.append(contentsOf: encoded)
        }
        return out
    }

    static func writeText(_ text: String, into raw: UnsafeMutableRawPointer, capacity: Int) {
        let destination = raw.assumingMemoryBound(to: CChar.self)
        destination.update(repeating: 0, count: capacity)
        let bytes = clampedUTF8(text, capacity: capacity)
        for (offset, byte) in bytes.enumerated() {
            destination[offset] = CChar(bitPattern: byte)
        }
    }

    static func writeInts(_ values: [Int], into raw: UnsafeMutableRawPointer, capacity: Int)
        -> Int32
    {
        let destination = raw.assumingMemoryBound(to: Int32.self)
        destination.update(repeating: 0, count: capacity)
        let count = min(values.count, capacity)
        for index in 0..<count {
            destination[index] = Int32(clamping: values[index])
        }
        return Int32(count)
    }

    /// Wraps caller memory without copying. The returned value must not outlive the call.
    static func borrow(_ pointer: UnsafePointer<UInt8>, _ count: Int) -> Data {
        Data(
            bytesNoCopy: UnsafeMutableRawPointer(mutating: pointer), count: count,
            deallocator: .none)
    }

    /// Writes `bytes` when `capacity` allows and always reports the required size, so a
    /// caller can size a buffer with one probing call.
    static func emit(_ bytes: Data, into out: UnsafeMutablePointer<UInt8>?, capacity: Int) -> Int64
    {
        if let out, capacity >= bytes.count {
            bytes.copyBytes(to: out, count: bytes.count)
        }
        return Int64(bytes.count)
    }
}

@_cdecl("opc_relay_protocol_info")
func opc_relay_protocol_info(_ out: UnsafeMutablePointer<OpcRelayProtocolInfo>?) -> Int32 {
    guard let out else { return OPC_RELAY_ERR_NULL }
    out.pointee.version = Int32(WatcherRelayProtocol.version)
    out.pointee.hevc_codec = Int32(WatcherRelayProtocol.hevcCodec)
    out.pointee.max_payload_bytes = Int32(clamping: WatcherRelayProtocol.maximumPayloadBytes)
    out.pointee.framing_header_bytes = Int32(WatcherRelayFraming.headerBytes)
    out.pointee.max_retries = Int32(WatcherRelayRecovery.maximumRetries)
    out.pointee.join_timeout_ms = Int32(WatcherRelayRecovery.joinTimeout * 1000)
    out.pointee.silence_timeout_ms = Int32(WatcherRelayRecovery.silenceTimeout * 1000)
    out.pointee.reserved = 0
    withUnsafeMutablePointer(to: &out.pointee.service_type) {
        DesktopFacade.writeText(
            WatcherRelayProtocol.serviceType, into: UnsafeMutableRawPointer($0),
            capacity: DesktopFacade.textCapacity)
    }
    withUnsafeMutablePointer(to: &out.pointee.txt_camera) {
        DesktopFacade.writeText(
            WatcherRelayProtocol.txtCamera, into: UnsafeMutableRawPointer($0), capacity: 8)
    }
    withUnsafeMutablePointer(to: &out.pointee.txt_watchable) {
        DesktopFacade.writeText(
            WatcherRelayProtocol.txtWatchable, into: UnsafeMutableRawPointer($0), capacity: 8)
    }
    return OPC_RELAY_OK
}

@_cdecl("opc_desktop_core_version")
func opc_desktop_core_version(_ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int) -> Int64 {
    #if arch(arm64)
        let arch = "arm64"
    #elseif arch(x86_64)
        let arch = "x86_64"
    #else
        let arch = "unknown"
    #endif
    let text = "OpenPocketViewCore swift-desktop/\(arch)"
    return DesktopFacade.emit(Data(text.utf8), into: out, capacity: capacity)
}
