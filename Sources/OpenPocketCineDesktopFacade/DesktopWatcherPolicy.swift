import COpcDesktop
import Foundation
import OpenPocketViewCore

/// Per-join watcher policy: the bounded retry ladder plus the delivery-delay guard.
///
/// The host owns the clock and the socket; it must not invent its own deadlines. Both
/// pieces reset together on a join, which is why they share one handle.
private final class WatcherPolicyBox {
    var recovery: WatcherRelayRecovery
    var freshness = WatcherRelayFrameFreshness()

    init(now: TimeInterval) {
        recovery = WatcherRelayRecovery(now: now)
    }
}

private func policy(_ handle: UnsafeMutableRawPointer?) -> WatcherPolicyBox? {
    guard let handle else { return nil }
    return Unmanaged<WatcherPolicyBox>.fromOpaque(handle).takeUnretainedValue()
}

private func code(for action: WatcherRelayRecovery.Action) -> Int32 {
    switch action {
    case .none: return OPC_RELAY_ACTION_NONE
    case .reconnect: return OPC_RELAY_ACTION_RECONNECT
    case .exhausted: return OPC_RELAY_ACTION_EXHAUSTED
    }
}

@_cdecl("opc_relay_policy_create")
func opc_relay_policy_create(_ now: Double) -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(WatcherPolicyBox(now: now)).toOpaque()
}

@_cdecl("opc_relay_policy_destroy")
func opc_relay_policy_destroy(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<WatcherPolicyBox>.fromOpaque(handle).release()
}

/// The host accepted the join. Starts the silence and first-picture deadlines.
@_cdecl("opc_relay_policy_connected")
func opc_relay_policy_connected(_ handle: UnsafeMutableRawPointer?, _ now: Double) {
    guard let box = policy(handle) else { return }
    box.recovery.connected(now: now)
    box.freshness = WatcherRelayFrameFreshness()
}

@_cdecl("opc_relay_policy_received")
func opc_relay_policy_received(
    _ handle: UnsafeMutableRawPointer?, _ now: Double, _ picture: Int32
) {
    policy(handle)?.recovery.received(now: now, picture: picture != 0)
}

@_cdecl("opc_relay_policy_disconnected")
func opc_relay_policy_disconnected(_ handle: UnsafeMutableRawPointer?, _ now: Double) -> Int32 {
    guard let box = policy(handle) else { return OPC_RELAY_ACTION_NONE }
    return code(for: box.recovery.disconnected(now: now))
}

@_cdecl("opc_relay_policy_tick")
func opc_relay_policy_tick(_ handle: UnsafeMutableRawPointer?, _ now: Double) -> Int32 {
    guard let box = policy(handle) else { return OPC_RELAY_ACTION_NONE }
    return code(for: box.recovery.tick(now: now))
}

@_cdecl("opc_relay_policy_stop")
func opc_relay_policy_stop(_ handle: UnsafeMutableRawPointer?) {
    policy(handle)?.recovery.stop()
}

@_cdecl("opc_relay_policy_retry_count")
func opc_relay_policy_retry_count(_ handle: UnsafeMutableRawPointer?) -> Int32 {
    Int32(policy(handle)?.recovery.retryCount ?? 0)
}

/// Non-zero while a reconnect is waiting out its backoff.
@_cdecl("opc_relay_policy_retry_pending")
func opc_relay_policy_retry_pending(_ handle: UnsafeMutableRawPointer?) -> Int32 {
    policy(handle)?.recovery.retryAt == nil ? 0 : 1
}

/// Non-zero once delivery delay has grown past the relay's tolerance. Reconnecting is
/// the watcher's only remedy — it never asks the camera for a keyframe.
@_cdecl("opc_relay_policy_is_falling_behind")
func opc_relay_policy_is_falling_behind(
    _ handle: UnsafeMutableRawPointer?, _ hasEncodedAt: Int32, _ encodedAt: Double,
    _ receivedAt: Double
) -> Int32 {
    guard let box = policy(handle) else { return 0 }
    let stamp: TimeInterval? = hasEncodedAt != 0 ? encodedAt : nil
    return box.freshness.isFallingBehind(encodedAt: stamp, receivedAt: receivedAt) ? 1 : 0
}
