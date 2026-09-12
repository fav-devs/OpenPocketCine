import COpcDesktop
import Foundation
import OpenPocketViewCore

// Bluetooth pairing and the Wi-Fi handover.
//
// These are the two steps a PC does differently from a phone, and the two where
// guessing is most tempting: the GATT map, the pairing handshake's replies, and how long
// to keep trying a Wi-Fi join are all already settled in the core and in the protocol
// notes. The shell supplies a Bluetooth stack and a way to run a join; it supplies no
// opinions.

/// The GATT identifiers, newline separated: service, notify characteristic, write
/// characteristic, client-configuration descriptor.
@_cdecl("opc_ble_gatt_uuids")
func opc_ble_gatt_uuids(_ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int) -> Int64 {
    let joined = [
        BleConstants.serviceFFF0, BleConstants.charFFF4, BleConstants.charFFF5,
        BleConstants.cccd,
    ].joined(separator: "\n")
    return DesktopFacade.emit(Data(joined.utf8), into: out, capacity: capacity)
}

/// Reads a camera's model out of its advertisement.
///
/// `modelId` is `-1` when the advert does not carry one — the name is the fallback, and
/// that is the shell's to pass on.
@_cdecl("opc_ble_advert_decode")
func opc_ble_advert_decode(
    _ payload: UnsafePointer<UInt8>?, _ count: Int, _ modelId: UnsafeMutablePointer<Int32>?,
    _ newFormat: UnsafeMutablePointer<Int32>?, _ rawProductType: UnsafeMutablePointer<Int32>?
) -> Int32 {
    guard let payload, count >= 0 else { return OPC_RELAY_ERR_NULL }
    let decoded = BleAdvert.decode(Array(DesktopFacade.borrow(payload, count)))
    modelId?.pointee = Int32(decoded.modelId ?? -1)
    newFormat?.pointee = decoded.newFormat ? 1 : 0
    rawProductType?.pointee = Int32(decoded.rawProductType ?? -1)
    return OPC_RELAY_OK
}

/// Reassembles BLE notifications into whole DUML frames.
///
/// A notification is not a frame: the camera splits replies across several, and a naive
/// reader sees truncated payloads rather than an error.
private final class AssemblerBox {
    var assembler = DumlNotificationAssembler()
    /// The blob the last append produced, so a caller may probe its size and then fetch
    /// it. Appending twice would consume the notification and lose the frames.
    var pending: [UInt8] = []
}

private func assemblerBox(_ handle: UnsafeMutableRawPointer?) -> AssemblerBox? {
    guard let handle else { return nil }
    return Unmanaged<AssemblerBox>.fromOpaque(handle).takeUnretainedValue()
}

@_cdecl("opc_ble_assembler_create")
func opc_ble_assembler_create() -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(AssemblerBox()).toOpaque()
}

@_cdecl("opc_ble_assembler_destroy")
func opc_ble_assembler_destroy(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<AssemblerBox>.fromOpaque(handle).release()
}

/// Appends one notification and reports how many bytes its frames need.
///
/// Frames are held for `opc_ble_assembler_take`; appending again would consume the next
/// notification rather than re-report this one.
@_cdecl("opc_ble_assembler_append")
func opc_ble_assembler_append(
    _ handle: UnsafeMutableRawPointer?, _ bytes: UnsafePointer<UInt8>?, _ count: Int,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let box = assemblerBox(handle), let bytes, count >= 0 else {
        return Int64(OPC_RELAY_ERR_NULL)
    }
    let frames = box.assembler.append(Array(DesktopFacade.borrow(bytes, count)))
    var blob: [UInt8] = [UInt8(frames.count & 0xFF), UInt8((frames.count >> 8) & 0xFF)]
    for frame in frames {
        let packed: [UInt8] = [
            frame.sender, frame.receiver,
            UInt8(frame.seq & 0xFF), UInt8((frame.seq >> 8) & 0xFF),
            frame.flags, frame.cmdSet, frame.cmdId,
        ] + frame.payload
        blob.append(UInt8(packed.count & 0xFF))
        blob.append(UInt8((packed.count >> 8) & 0xFF))
        blob.append(contentsOf: packed)
    }
    box.pending = blob
    return DesktopFacade.emit(Data(blob), into: out, capacity: capacity)
}

/// Copies the frames the last append produced.
@_cdecl("opc_ble_assembler_take")
func opc_ble_assembler_take(
    _ handle: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let box = assemblerBox(handle) else { return Int64(OPC_RELAY_ERR_NULL) }
    return DesktopFacade.emit(Data(box.pending), into: out, capacity: capacity)
}

/// Pulls the string out of a status reply — how the Wi-Fi name and password arrive.
@_cdecl("opc_duml_status_string")
func opc_duml_status_string(
    _ payload: UnsafePointer<UInt8>?, _ count: Int, _ out: UnsafeMutablePointer<UInt8>?,
    _ capacity: Int
) -> Int64 {
    guard let payload, count >= 0 else { return Int64(OPC_RELAY_ERR_NULL) }
    let text = Duml.unpackStatusString(Array(DesktopFacade.borrow(payload, count)))
    return DesktopFacade.emit(Data(text.utf8), into: out, capacity: capacity)
}

/// `0x07/0x45` SetPairingPIN, which is where a first-time pairing begins.
@_cdecl("opc_pair_set_pin")
func opc_pair_set_pin(
    _ pin: UnsafePointer<CChar>?, _ identifier: UnsafePointer<CChar>?,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let pin else { return Int64(OPC_RELAY_ERR_NULL) }
    let frame: Duml.Frame
    if let identifier {
        frame = Commands.setPairingPin(
            pin: String(cString: pin), identifier: String(cString: identifier))
    } else {
        frame = Commands.setPairingPin(pin: String(cString: pin))
    }
    return DesktopFacade.emit(Data(Duml.encode(frame)), into: out, capacity: capacity)
}

/// Answers the camera's own `0x07/0x46` approval request. Echo its sequence.
@_cdecl("opc_pair_approval_ack")
func opc_pair_approval_ack(
    _ seq: UInt16, _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    DesktopFacade.emit(
        Data(Duml.encode(Commands.pairApprovalAck(seq: seq))), into: out, capacity: capacity)
}

/// `0x53/0x10`. The camera answers and wakes its access point.
@_cdecl("opc_pair_wake_access_point")
func opc_pair_wake_access_point(_ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int) -> Int64 {
    DesktopFacade.emit(
        Data(Duml.encode(Commands.session5310())), into: out, capacity: capacity)
}

// ---- Wi-Fi join policy ------------------------------------------------------

/// How long a join may keep trying before the operator is told it failed.
@_cdecl("opc_join_deadline_seconds")
func opc_join_deadline_seconds() -> Double {
    CameraSoftAPSwitch.joinDeadlineSeconds
}

/// How long to pause between attempts.
@_cdecl("opc_join_retry_pause_seconds")
func opc_join_retry_pause_seconds() -> Double {
    CameraSoftAPSwitch.joinRetryPauseSeconds
}

@_cdecl("opc_join_should_retry")
func opc_join_should_retry(_ secondsLeft: Double) -> Int32 {
    CameraSoftAPSwitch.shouldRetryJoin(secondsLeft: secondsLeft) ? 1 : 0
}

/// Non-zero when the machine is on the network it was asked to join.
///
/// An unknown current network counts as success: some platforms hide it, and refusing
/// to proceed on that basis strands an operator who is in fact connected.
@_cdecl("opc_join_is_on_target")
func opc_join_is_on_target(
    _ currentSSID: UnsafePointer<CChar>?, _ target: UnsafePointer<CChar>?
) -> Int32 {
    guard let target else { return 0 }
    let current = currentSSID.map { String(cString: $0) }
    return CameraSoftAPSwitch.isOnTarget(
        currentSSID: current, target: String(cString: target)) ? 1 : 0
}

/// The network to disconnect from first, or empty when there is nothing to leave.
@_cdecl("opc_join_ssid_to_kick")
func opc_join_ssid_to_kick(
    _ currentSSID: UnsafePointer<CChar>?, _ target: UnsafePointer<CChar>?,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let target else { return Int64(OPC_RELAY_ERR_NULL) }
    let current = currentSSID.map { String(cString: $0) }
    let kick =
        CameraSoftAPSwitch.ssidToKick(currentSSID: current, target: String(cString: target)) ?? ""
    return DesktopFacade.emit(Data(kick.utf8), into: out, capacity: capacity)
}

/// The operator-facing hint about the camera's 5 GHz band.
@_cdecl("opc_join_frequency_hint")
func opc_join_frequency_hint(_ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int) -> Int64 {
    DesktopFacade.emit(
        Data(CameraSoftAPSwitch.frequencyHint.utf8), into: out, capacity: capacity)
}
