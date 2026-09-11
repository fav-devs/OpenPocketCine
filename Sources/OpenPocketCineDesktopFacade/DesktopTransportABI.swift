import COpcDesktop
import Foundation
import OpenPocketViewCore

// The UDP datalink's byte math, which stays in the core.
//
// The shell owns the socket and the 40 Hz clock. What goes in an acknowledgement — and
// in particular which window cursors may move and when — does not: telemetry rewinding
// a cursor after the first video packet is what stops the picture while the HUD keeps
// running, and that rule lives in `DumlTransport.AckWindows`.

@_cdecl("opc_duml_transport_header")
func opc_duml_transport_header(
    _ pktType: UInt8, _ payloadLength: Int, _ sessionId: UInt16, _ seq: UInt16,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard payloadLength >= 0 else { return Int64(OPC_RELAY_ERR_OUT_OF_RANGE) }
    let header = DumlTransport.transportHeader(
        pktType: pktType, payloadLen: payloadLength, sessionId: sessionId, seq: seq)
    return DesktopFacade.emit(Data(header), into: out, capacity: capacity)
}

@_cdecl("opc_duml_routing_header")
func opc_duml_routing_header(
    _ seq: UInt16, _ commandCounter: UInt8, _ drone: Int32,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    let header = DumlTransport.routingHeader(
        seq: seq, cmdCounter: commandCounter, drone: drone != 0)
    return DesktopFacade.emit(Data(header), into: out, capacity: capacity)
}

/// The 48-byte session open. `baseSeq` must be a fresh 8-aligned value per connect — a
/// fixed one can wedge the camera.
@_cdecl("opc_duml_handshake")
func opc_duml_handshake(
    _ sessionId: UInt16, _ seq: UInt16, _ baseSeq: UInt16,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    let datagram = DumlTransport.handshakeDatagram(
        sessionId: sessionId, seq: seq, baseSeq: baseSeq)
    return DesktopFacade.emit(Data(datagram), into: out, capacity: capacity)
}

@_cdecl("opc_duml_is_handshake")
func opc_duml_is_handshake(_ datagram: UnsafePointer<UInt8>?, _ count: Int) -> Int32 {
    guard let datagram, count >= 0 else { return OPC_RELAY_ERR_NULL }
    return DumlTransport.isHandshake(Array(DesktopFacade.borrow(datagram, count))) ? 1 : 0
}

@_cdecl("opc_duml_transport_seq")
func opc_duml_transport_seq(
    _ datagram: UnsafePointer<UInt8>?, _ count: Int, _ out: UnsafeMutablePointer<UInt16>?
) -> Int32 {
    guard let datagram, let out, count >= 0 else { return OPC_RELAY_ERR_NULL }
    guard let seq = DumlTransport.transportSeq(Array(DesktopFacade.borrow(datagram, count)))
    else { return OPC_RELAY_NEED_MORE }
    out.pointee = seq
    return OPC_RELAY_OK
}

/// Pulls every complete DUML frame out of a datagram's payload, as
/// `[u16le count]([u16le length][sender][receiver][seq lo][seq hi][flags][set][id][payload])…`.
@_cdecl("opc_duml_scan_frames")
func opc_duml_scan_frames(
    _ raw: UnsafePointer<UInt8>?, _ count: Int, _ out: UnsafeMutablePointer<UInt8>?,
    _ capacity: Int
) -> Int64 {
    guard let raw, count >= 0 else { return Int64(OPC_RELAY_ERR_NULL) }
    let frames = DumlTransport.scanFrames(Array(DesktopFacade.borrow(raw, count)))
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
    return DesktopFacade.emit(Data(blob), into: out, capacity: capacity)
}

/// Encodes one DUML frame, CRC included.
@_cdecl("opc_duml_encode")
func opc_duml_encode(
    _ sender: UInt8, _ receiver: UInt8, _ seq: UInt16, _ flags: UInt8, _ cmdSet: UInt8,
    _ cmdId: UInt8, _ payload: UnsafePointer<UInt8>?, _ payloadCount: Int,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard payloadCount >= 0 else { return Int64(OPC_RELAY_ERR_OUT_OF_RANGE) }
    let body: [UInt8] =
        payload.map { Array(DesktopFacade.borrow($0, payloadCount)) } ?? []
    let frame = Duml.Frame(
        sender: sender, receiver: receiver, seq: seq, flags: flags, cmdSet: cmdSet,
        cmdId: cmdId, payload: body)
    return DesktopFacade.emit(Data(Duml.encode(frame)), into: out, capacity: capacity)
}

/// Holds the three window cursors across a session.
///
/// Group 0 (video) is deliberately not handled by `AckWindows.advancing`: the core
/// tracks groups 1 and 2 there, and the shell owns group 0 because it is the latest
/// pktType-`0x02` transport sequence. Two flags, not one, mirror the iOS driver:
/// `hasGroup0` says the cursor has any value at all, and `hasVideoSeq` says a real
/// video packet set it. Telemetry may seed the cursor before the first video packet
/// but must never rewind it afterwards — that is the bug that closes HEVC while the
/// HUD keeps running.
private final class AckBox {
    var windows: DumlTransport.AckWindows
    var videoCursor: UInt16 = 0
    var hasGroup0 = false
    var hasVideoSeq = false

    init(_ windows: DumlTransport.AckWindows) {
        self.windows = windows
    }
}

private func ackBox(_ handle: UnsafeMutableRawPointer?) -> AckBox? {
    guard let handle else { return nil }
    return Unmanaged<AckBox>.fromOpaque(handle).takeUnretainedValue()
}

/// A fresh set of cursors. The handshake base is supplied per ACK instead of here, so a
/// rebuilt session can keep the same handle.
@_cdecl("opc_ack_create")
func opc_ack_create() -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(AckBox(DumlTransport.AckWindows())).toOpaque()
}

@_cdecl("opc_ack_destroy")
func opc_ack_destroy(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<AckBox>.fromOpaque(handle).release()
}

/// Folds one received datagram into the cursors, applying the core's rewind rules.
@_cdecl("opc_ack_advance")
func opc_ack_advance(
    _ handle: UnsafeMutableRawPointer?, _ datagram: UnsafePointer<UInt8>?, _ count: Int
) -> Int32 {
    guard let box = ackBox(handle), let datagram, count >= 0 else { return OPC_RELAY_ERR_NULL }
    let bytes = Array(DesktopFacade.borrow(datagram, count))
    // Groups 1 and 2 follow the core's rules exactly.
    box.windows = box.windows.advancing(datagram: bytes)

    guard bytes.count >= 8 else { return OPC_RELAY_OK }
    switch bytes[6] {
    case DumlTransport.PktType.video.rawValue:
        if let seq = DumlTransport.transportSeq(bytes) {
            box.videoCursor = seq
            box.hasVideoSeq = true
            box.hasGroup0 = true
        }
    case DumlTransport.PktType.telemetry.rawValue:
        guard
            DumlTransport.AckWindows.shouldSeedVideoCursorFromTelemetry(
                hasVideoSeq: box.hasVideoSeq),
            let seeded = DumlTransport.ackWindows(fromTelemetry: bytes)
        else { break }
        box.videoCursor = seeded.video
        box.hasGroup0 = true
    default:
        break
    }
    return OPC_RELAY_OK
}

/// Non-zero once a real pktType-`0x02` has moved group 0 — the shell's "first picture".
@_cdecl("opc_ack_saw_video")
func opc_ack_saw_video(_ handle: UnsafeMutableRawPointer?) -> Int32 {
    ackBox(handle)?.hasVideoSeq == true ? 1 : 0
}

@_cdecl("opc_ack_read")
func opc_ack_read(
    _ handle: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<OpcAckWindows>?
) -> Int32 {
    guard let box = ackBox(handle), let out else { return OPC_RELAY_ERR_NULL }
    out.pointee.video = UInt32(box.videoCursor)
    out.pointee.acked_data = UInt32(box.windows.ackedData)
    out.pointee.extra = UInt32(box.windows.extra)
    out.pointee.has_acked_data = box.windows.hasAckedData ? 1 : 0
    out.pointee.has_extra = box.windows.hasExtra ? 1 : 0
    return OPC_RELAY_OK
}

/// The pktType-`0x04` payload the pump sends 40 times a second.
///
/// A cursor that has never been seen sends `baseSeq` rather than zero. Zero is a real
/// 8-aligned sequence, so "unseen" and "zero" have to stay distinguishable.
@_cdecl("opc_ack_payload")
func opc_ack_payload(
    _ handle: UnsafeMutableRawPointer?, _ baseSeq: UInt16, _ out: UnsafeMutablePointer<UInt8>?,
    _ capacity: Int
) -> Int64 {
    guard let box = ackBox(handle) else { return Int64(OPC_RELAY_ERR_NULL) }
    let cursor = DumlTransport.AckWindows.windowCursor
    let payload = DumlTransport.ackPayload(
        peerCursor: cursor(box.videoCursor, box.hasGroup0, baseSeq),
        ackedDataCursor: cursor(box.windows.ackedData, box.windows.hasAckedData, baseSeq),
        extraCursor: cursor(box.windows.extra, box.windows.hasExtra, baseSeq))
    return DesktopFacade.emit(Data(payload), into: out, capacity: capacity)
}
