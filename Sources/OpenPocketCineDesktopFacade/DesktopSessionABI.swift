import COpcDesktop
import Foundation
import OpenPocketViewCore

// Reassembly and the SoftAP path gates.
//
// Both are policy the shells share. A desktop that guessed at either would be guessing
// at the two things that decide whether a picture ever appears: whether the socket is
// on-path, and how video packets become access units.

/// Wraps the core's reassembler for one session.
private final class DepacketizerBox {
    var depacketizer = HevcDepacketizer()
    /// The last completed unit, so a caller may probe its size and then fetch it.
    var pending: [UInt8]?
}

private func depacketizerBox(_ handle: UnsafeMutableRawPointer?) -> DepacketizerBox? {
    guard let handle else { return nil }
    return Unmanaged<DepacketizerBox>.fromOpaque(handle).takeUnretainedValue()
}

@_cdecl("opc_depacketizer_create")
func opc_depacketizer_create() -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(DepacketizerBox()).toOpaque()
}

@_cdecl("opc_depacketizer_destroy")
func opc_depacketizer_destroy(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<DepacketizerBox>.fromOpaque(handle).release()
}

/// Feeds one pktType-`0x02` payload.
///
/// Returns the access unit's byte count when this packet completed one, `0` when more
/// packets are needed, or a negative status. A completed unit is written into `out` when
/// `capacity` allows; call once with a null `out` to learn the size first.
@_cdecl("opc_depacketizer_feed")
func opc_depacketizer_feed(
    _ handle: UnsafeMutableRawPointer?, _ payload: UnsafePointer<UInt8>?, _ count: Int,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let box = depacketizerBox(handle), let payload, count >= 0 else {
        return Int64(OPC_RELAY_ERR_NULL)
    }
    guard let unit = box.depacketizer.feed(Array(DesktopFacade.borrow(payload, count))) else {
        return 0
    }
    // The unit is held so a caller that probed with a null buffer can fetch it next.
    box.pending = unit
    return DesktopFacade.emit(Data(unit), into: out, capacity: capacity)
}

/// Copies the access unit the last `feed` completed, for a caller that probed its size.
@_cdecl("opc_depacketizer_take")
func opc_depacketizer_take(
    _ handle: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let box = depacketizerBox(handle) else { return Int64(OPC_RELAY_ERR_NULL) }
    guard let pending = box.pending else { return 0 }
    return DesktopFacade.emit(Data(pending), into: out, capacity: capacity)
}

/// Drops partial state. Used on a session rebuild, which must not lose the ACK cursor.
@_cdecl("opc_depacketizer_reset")
func opc_depacketizer_reset(_ handle: UnsafeMutableRawPointer?) {
    guard let box = depacketizerBox(handle) else { return }
    box.depacketizer.reset()
    box.pending = nil
}

/// How many access units were abandoned incomplete — a packet-loss signal for the HUD.
@_cdecl("opc_depacketizer_dropped")
func opc_depacketizer_dropped(_ handle: UnsafeMutableRawPointer?) -> Int32 {
    Int32(depacketizerBox(handle)?.depacketizer.droppedIncomplete ?? 0)
}

/// The camera's fixed address and the port rules around it.
@_cdecl("opc_softap_remote_port")
func opc_softap_remote_port() -> UInt16 {
    CameraSoftAP.remotePort
}

@_cdecl("opc_softap_host")
func opc_softap_host(_ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int) -> Int64 {
    DesktopFacade.emit(Data(CameraSoftAP.host.utf8), into: out, capacity: capacity)
}

/// Non-zero when this local port is one the datalink may bind.
///
/// Binding the camera's own `9004` locally accepted telemetry and dropped every video
/// packet. An ephemeral port is the only correct choice.
@_cdecl("opc_softap_may_bind_local_port")
func opc_softap_may_bind_local_port(_ port: UInt16) -> Int32 {
    CameraSoftAP.isEphemeralLocalPort(port) ? 1 : 0
}

/// Non-zero when this IPv4 address means the machine is associated with the camera.
@_cdecl("opc_softap_is_associated")
func opc_softap_is_associated(_ ipv4: UnsafePointer<CChar>?) -> Int32 {
    guard let ipv4 else { return 0 }
    return CameraSoftAP.isAssociatedIPv4(String(cString: ipv4)) ? 1 : 0
}

/// Non-zero when any of the machine's addresses puts it on the camera's path.
/// `addresses` is newline separated.
@_cdecl("opc_softap_is_path_ready")
func opc_softap_is_path_ready(_ addresses: UnsafePointer<CChar>?) -> Int32 {
    guard let addresses else { return 0 }
    let list = String(cString: addresses)
        .split(separator: "\n", omittingEmptySubsequences: true)
        .map(String.init)
    return CameraSoftAP.isPathReady(localIPv4s: list) ? 1 : 0
}

/// Non-zero when this SSID looks like an Osmo's own access point.
@_cdecl("opc_softap_is_camera_ssid")
func opc_softap_is_camera_ssid(_ ssid: UnsafePointer<CChar>?) -> Int32 {
    guard let ssid else { return 0 }
    return CameraSoftAP.isOsmoSoftAPSSID(String(cString: ssid)) ? 1 : 0
}
