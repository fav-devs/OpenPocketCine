import COpcDesktop
import Foundation
import OpenPocketViewCore

// Keeping a live feed alive.
//
// A feed that freezes does not announce itself: the socket stays open, telemetry keeps
// arriving, and the picture simply stops. What to do about that — and in what order, and
// how long to wait between rungs — is `FeedWatchdog`, which both phones already run. The
// desktop shell supplies the observations and carries out the action; it decides
// neither.

/// Holds the ladder's state across ticks.
private final class WatchdogBox {
    var watchdog = FeedWatchdog()
}

private func watchdogBox(_ handle: UnsafeMutableRawPointer?) -> WatchdogBox? {
    guard let handle else { return nil }
    return Unmanaged<WatchdogBox>.fromOpaque(handle).takeUnretainedValue()
}

/// A negative age means the shell has never seen that thing. Zero is a real age.
private func age(_ seconds: Double) -> TimeInterval? {
    seconds < 0 ? nil : seconds
}

@_cdecl("opc_watchdog_create")
func opc_watchdog_create() -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(WatchdogBox()).toOpaque()
}

@_cdecl("opc_watchdog_destroy")
func opc_watchdog_destroy(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<WatchdogBox>.fromOpaque(handle).release()
}

/// Folds one observation in and returns the rung to act on, if any.
@_cdecl("opc_watchdog_tick")
func opc_watchdog_tick(
    _ handle: UnsafeMutableRawPointer?, _ snapshot: UnsafePointer<OpcWatchdogSnapshot>?
) -> Int32 {
    guard let box = watchdogBox(handle), let snapshot else { return OPC_WATCHDOG_NONE }
    let input = snapshot.pointee
    let action = box.watchdog.tick(
        FeedWatchdog.Snapshot(
            now: input.now,
            lastDecodedFrameAge: age(input.last_decoded_frame_age),
            lastVideoPacketAge: age(input.last_video_packet_age),
            lastAccessUnitAge: age(input.last_access_unit_age),
            lastStatusAge: age(input.last_status_age),
            flowHealthy: input.flow_healthy != 0,
            pathReady: input.path_ready != 0,
            hasFormat: input.has_format != 0,
            decoderFailed: input.decoder_failed != 0,
            live: input.live != 0,
            sawPicture: input.saw_picture != 0,
            tcpPokeReady: input.tcp_poke_ready != 0,
            displayedImageRemoved: input.displayed_image_removed != 0,
            lastBleNotifyAge: age(input.last_ble_notify_age),
            secondsSinceLastRebuild: age(input.seconds_since_last_rebuild),
            hadVideo: input.had_video != 0,
            secondsSinceLastEnable: age(input.seconds_since_last_enable),
            secondsSinceFocusTrackSet: age(input.seconds_since_focus_track_set),
            secondsSinceZoomSet: age(input.seconds_since_zoom_set),
            secondsSinceGimbalThrow: age(input.seconds_since_gimbal_throw),
            secondsSinceCameraSet: age(input.seconds_since_camera_set)
        ))
    switch action {
    case .none: return OPC_WATCHDOG_NONE
    case .resendLiveViewEnable: return OPC_WATCHDOG_RESEND_ENABLE
    case .rebuildVTSession: return OPC_WATCHDOG_REBUILD_DECODER
    case .reopenDatalink: return OPC_WATCHDOG_REOPEN_DATALINK
    case .fullSessionRejoin: return OPC_WATCHDOG_FULL_REJOIN
    }
}

/// The rung the ladder is resting on, for diagnostics.
@_cdecl("opc_watchdog_stage")
func opc_watchdog_stage(
    _ handle: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let box = watchdogBox(handle) else { return Int64(OPC_RELAY_ERR_NULL) }
    return DesktopFacade.emit(
        Data(box.watchdog.stage.rawValue.utf8), into: out, capacity: capacity)
}

/// How long without a picture counts as a stall.
@_cdecl("opc_watchdog_stall_threshold")
func opc_watchdog_stall_threshold() -> Double {
    FeedWatchdog.stallThreshold
}
