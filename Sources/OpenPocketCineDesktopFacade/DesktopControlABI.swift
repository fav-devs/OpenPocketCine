import COpcDesktop
import Foundation
import OpenPocketViewCore

/// Live-control policy for the desktop shell: the SET mailbox, which opcodes it
/// governs, and the zoom rules per body.
///
/// The Rust session drives the clock — when to retransmit, when to settle — and
/// asks here what each moment means. The mailbox itself is the same
/// `CameraSetMailbox` the phones run, one instance per datalink.

// MARK: - Opcodes

/// `set << 8 | cmd`, the key every mailbox call takes.
@_cdecl("opc_duml_opcode_key")
func opc_duml_opcode_key(_ set: Int32, _ cmd: Int32) -> Int32 {
    guard (0...255).contains(set), (0...255).contains(cmd) else { return -1 }
    return Int32(Duml.opcodeKey(set: UInt8(set), cmd: UInt8(cmd)))
}

/// Whether a key is one of the live-control SETs the mailbox governs.
@_cdecl("opc_duml_is_live_control")
func opc_duml_is_live_control(_ key: Int32) -> Int32 {
    guard (0...0xFFFF).contains(key) else { return 0 }
    let set = UInt8(truncatingIfNeeded: key >> 8)
    let cmd = UInt8(truncatingIfNeeded: key)
    return Duml.isLiveCameraControl(set: set, cmd: cmd) ? 1 : 0
}

// MARK: - The mailbox

private final class MailboxBox {
    var mailbox = CameraSetMailbox()
}

private func mailbox(_ handle: UnsafeMutableRawPointer?) -> MailboxBox? {
    guard let handle else { return nil }
    return Unmanaged<MailboxBox>.fromOpaque(handle).takeUnretainedValue()
}

private func key16(_ key: Int32) -> UInt16? {
    (0...0xFFFF).contains(key) ? UInt16(key) : nil
}

@_cdecl("opc_mailbox_new")
func opc_mailbox_new() -> UnsafeMutableRawPointer? {
    Unmanaged.passRetained(MailboxBox()).toOpaque()
}

@_cdecl("opc_mailbox_destroy")
func opc_mailbox_destroy(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<MailboxBox>.fromOpaque(handle).release()
}

@_cdecl("opc_mailbox_reset")
func opc_mailbox_reset(_ handle: UnsafeMutableRawPointer?) {
    mailbox(handle)?.mailbox.reset()
}

/// `OPC_MAILBOX_LAUNCH` to transmit now, `OPC_MAILBOX_COALESCE` to keep it pending.
@_cdecl("opc_mailbox_offer")
func opc_mailbox_offer(
    _ handle: UnsafeMutableRawPointer?, _ key: Int32, _ urgent: Int32, _ now: Double
) -> Int32 {
    guard let box = mailbox(handle), let key = key16(key) else { return OPC_MAILBOX_LAUNCH }
    switch box.mailbox.offer(key: key, urgent: urgent != 0, now: now) {
    case .launch: return OPC_MAILBOX_LAUNCH
    case .coalescePending: return OPC_MAILBOX_COALESCE
    }
}

@_cdecl("opc_mailbox_begin_launch")
func opc_mailbox_begin_launch(_ handle: UnsafeMutableRawPointer?, _ key: Int32, _ now: Double) {
    guard let box = mailbox(handle), let key = key16(key) else { return }
    box.mailbox.beginLaunch(key: key, now: now)
}

@_cdecl("opc_mailbox_note_transmit")
func opc_mailbox_note_transmit(_ handle: UnsafeMutableRawPointer?, _ key: Int32, _ seq: Int32) {
    guard let box = mailbox(handle), let key = key16(key), let seq = key16(seq) else { return }
    box.mailbox.noteTransmit(key: key, seq: seq)
}

/// One of `OPC_MAILBOX_ACK_*`.
@_cdecl("opc_mailbox_decide_ack")
func opc_mailbox_decide_ack(_ handle: UnsafeMutableRawPointer?, _ key: Int32, _ seq: Int32)
    -> Int32
{
    guard let box = mailbox(handle), let key = key16(key), let seq = key16(seq) else {
        return OPC_MAILBOX_ACK_DROP_UNKNOWN
    }
    switch box.mailbox.decideAck(key: key, seq: seq) {
    case .accept: return OPC_MAILBOX_ACK_ACCEPT
    case .acceptLate: return OPC_MAILBOX_ACK_ACCEPT_LATE
    case .dropSuperseded: return OPC_MAILBOX_ACK_DROP_SUPERSEDED
    case .dropUnknown: return OPC_MAILBOX_ACK_DROP_UNKNOWN
    }
}

/// One of `OPC_MAILBOX_TIMEOUT_*`, after the settle window with no ACK.
@_cdecl("opc_mailbox_timeout")
func opc_mailbox_timeout(
    _ handle: UnsafeMutableRawPointer?, _ key: Int32, _ subscribeMatches: Int32
) -> Int32 {
    guard let box = mailbox(handle), let key = key16(key) else { return OPC_MAILBOX_TIMEOUT_IDLE }
    switch box.mailbox.timeout(key: key, subscribeMatches: subscribeMatches != 0) {
    case .subscribeMatches: return OPC_MAILBOX_TIMEOUT_SUBSCRIBE_MATCHES
    case .waitLate: return OPC_MAILBOX_TIMEOUT_WAIT_LATE
    case .launchPending: return OPC_MAILBOX_TIMEOUT_LAUNCH_PENDING
    case .idle: return OPC_MAILBOX_TIMEOUT_IDLE
    }
}

/// One of `OPC_MAILBOX_PENDING_*`: whether the pending SET for `key` may go now.
@_cdecl("opc_mailbox_pending_launch")
func opc_mailbox_pending_launch(_ handle: UnsafeMutableRawPointer?, _ key: Int32, _ now: Double)
    -> Int32
{
    guard let box = mailbox(handle), let key = key16(key) else { return OPC_MAILBOX_PENDING_NONE }
    switch box.mailbox.pendingLaunch(key: key, now: now) {
    case .immediate: return OPC_MAILBOX_PENDING_IMMEDIATE
    case .afterHold: return OPC_MAILBOX_PENDING_AFTER_HOLD
    case .none: return OPC_MAILBOX_PENDING_NONE
    }
}

@_cdecl("opc_mailbox_hold_remaining")
func opc_mailbox_hold_remaining(_ handle: UnsafeMutableRawPointer?, _ key: Int32, _ now: Double)
    -> Double
{
    guard let box = mailbox(handle), let key = key16(key) else { return 0 }
    return box.mailbox.holdRemaining(key: key, now: now)
}

/// Whether `key` pipelines while a generation is open (the zoom slider does).
@_cdecl("opc_mailbox_pipelines")
func opc_mailbox_pipelines(_ key: Int32) -> Int32 {
    guard let key = key16(key) else { return 0 }
    return CameraSetMailbox.pipelinesWhileOpen(key) ? 1 : 0
}

// MARK: - Zoom

/// The chip stops for this body, format and shooting mode, as operator factors.
/// Writes up to `capacity` doubles and returns how many there are.
@_cdecl("opc_zoom_stops")
func opc_zoom_stops(
    _ modelId: Int32, _ resolution: Int32, _ shootingMode: Int32,
    _ out: UnsafeMutablePointer<Double>?, _ capacity: Int
) -> Int32 {
    let model = CameraModel.resolve(modelId: Int(modelId), name: nil)
    let format = (0...255).contains(resolution) ? VideoResolution(rawValue: UInt8(resolution)) : nil
    let stops = model.activeZoomStops(resolution: format, shootingMode: Int(shootingMode))
    if let out {
        for (index, stop) in stops.prefix(capacity).enumerated() {
            out[index] = stop
        }
    }
    return Int32(stops.count)
}

/// What a zoom to `factor` needs first: nothing, a colour hop (D-Log2 rejects every
/// zoom SET; `outMode` gets the mode to set), or nothing at all because the body is
/// rolling and will not change colour.
@_cdecl("opc_zoom_hop")
func opc_zoom_hop(
    _ factor: Double, _ colorMode: Int32, _ isRecording: Int32,
    _ outMode: UnsafeMutablePointer<Int32>?
) -> Int32 {
    let current = (0...255).contains(colorMode) ? ColorMode(rawValue: UInt8(colorMode)) : nil
    if CamFov.zoomNeedsColorHopWhileRecording(
        factor: factor, current: current, isRecording: isRecording != 0)
    {
        return OPC_ZOOM_HOP_BLOCKED
    }
    guard let next = CamFov.colorMode(forZoom: factor, current: current) else {
        return OPC_ZOOM_HOP_NONE
    }
    outMode?.pointee = Int32(next.rawValue)
    return OPC_ZOOM_HOP_COLOR
}

/// Whether a zoom parked at `factor` is wide enough to put D-Log2 back.
@_cdecl("opc_zoom_restore_dlog2")
func opc_zoom_restore_dlog2(_ factor: Double) -> Int32 {
    CamFov.shouldRestoreDLog2(factor: factor) ? 1 : 0
}

// MARK: - Tracking and focus

/// Reads a `0x02/0xA5` poll reply. `OPC_TRACKING_LOCKED_BOX` also writes the subject
/// box as normalised `x, y, width, height` into `outBox`.
@_cdecl("opc_tracking_poll")
func opc_tracking_poll(
    _ payload: UnsafePointer<UInt8>?, _ count: Int, _ outBox: UnsafeMutablePointer<Float>?
) -> Int32 {
    guard let payload, count >= 0 else { return OPC_TRACKING_UNKNOWN }
    let bytes = [UInt8](UnsafeBufferPointer(start: payload, count: count))
    switch TrackingPoll.parse(bytes) {
    case .idle:
        return OPC_TRACKING_IDLE
    case .locked(let box):
        guard let box else { return OPC_TRACKING_LOCKED }
        if let outBox {
            outBox[0] = Float(box.x)
            outBox[1] = Float(box.y)
            outBox[2] = Float(box.width)
            outBox[3] = Float(box.height)
        }
        return OPC_TRACKING_LOCKED_BOX
    case nil:
        return OPC_TRACKING_UNKNOWN
    }
}

/// Whether this body takes Mimo's tap-to-focus burst (the Nano does not).
@_cdecl("opc_model_supports_tap_focus")
func opc_model_supports_tap_focus(_ modelId: Int32) -> Int32 {
    CameraModel.resolve(modelId: Int(modelId), name: nil).supportsTapFocus ? 1 : 0
}
