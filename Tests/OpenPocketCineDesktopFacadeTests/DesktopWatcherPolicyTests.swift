import COpcDesktop
import Foundation
import OpenPocketViewCore
import Testing

@testable import OpenPocketCineDesktopFacade

/// The host drives these from its own clock. Deadlines and the retry ladder stay in the
/// core so a desktop watcher cannot quietly retry harder than the phones do.
@Suite
struct DesktopWatcherPolicyTests {
    @Test
    func silenceSchedulesABoundedReconnect() {
        let handle = opc_relay_policy_create(0)
        defer { opc_relay_policy_destroy(handle) }
        opc_relay_policy_connected(handle, 0)

        #expect(opc_relay_policy_tick(handle, 1) == OPC_RELAY_ACTION_NONE)
        #expect(opc_relay_policy_retry_pending(handle) == 0)

        // Five seconds without a message is the silence deadline.
        #expect(opc_relay_policy_tick(handle, 6) == OPC_RELAY_ACTION_NONE)
        #expect(opc_relay_policy_retry_pending(handle) == 1)
        #expect(opc_relay_policy_retry_count(handle) == 1)

        // The first rung of the ladder is one second.
        #expect(opc_relay_policy_tick(handle, 6.5) == OPC_RELAY_ACTION_NONE)
        #expect(opc_relay_policy_tick(handle, 7.1) == OPC_RELAY_ACTION_RECONNECT)
        #expect(opc_relay_policy_retry_pending(handle) == 0)
    }

    @Test
    func trafficKeepsTheJoinAlive() {
        let handle = opc_relay_policy_create(0)
        defer { opc_relay_policy_destroy(handle) }
        opc_relay_policy_connected(handle, 0)
        for step in 1...20 {
            let now = Double(step)
            opc_relay_policy_received(handle, now, 1)
            #expect(opc_relay_policy_tick(handle, now) == OPC_RELAY_ACTION_NONE)
        }
        #expect(opc_relay_policy_retry_count(handle) == 0)
    }

    @Test
    func theRetryLadderIsExhaustedAfterThreeAttempts() {
        let handle = opc_relay_policy_create(0)
        defer { opc_relay_policy_destroy(handle) }
        opc_relay_policy_connected(handle, 0)

        var now = 0.0
        // Three rungs (1 s, 2 s, 4 s), then the watcher stops on its own.
        for expected in 1...3 {
            #expect(opc_relay_policy_disconnected(handle, now) == OPC_RELAY_ACTION_NONE)
            #expect(opc_relay_policy_retry_count(handle) == Int32(expected))
            now += 10
            #expect(opc_relay_policy_tick(handle, now) == OPC_RELAY_ACTION_RECONNECT)
        }
        #expect(opc_relay_policy_disconnected(handle, now) == OPC_RELAY_ACTION_EXHAUSTED)
        // A stopped policy stays stopped instead of starting a fourth attempt.
        #expect(opc_relay_policy_tick(handle, now + 100) == OPC_RELAY_ACTION_NONE)
    }

    @Test
    func stopEndsTheLadder() {
        let handle = opc_relay_policy_create(0)
        defer { opc_relay_policy_destroy(handle) }
        opc_relay_policy_connected(handle, 0)
        opc_relay_policy_stop(handle)
        #expect(opc_relay_policy_tick(handle, 1000) == OPC_RELAY_ACTION_NONE)
        #expect(opc_relay_policy_disconnected(handle, 1000) == OPC_RELAY_ACTION_NONE)
    }

    @Test
    func growingDeliveryDelayIsReportedWithoutASharedClock() {
        let handle = opc_relay_policy_create(0)
        defer { opc_relay_policy_destroy(handle) }
        opc_relay_policy_connected(handle, 0)
        // The first sample only establishes the baseline offset.
        #expect(opc_relay_policy_is_falling_behind(handle, 1, 0, 1) == 0)
        #expect(opc_relay_policy_is_falling_behind(handle, 1, 1, 1.5) == 0)
        #expect(opc_relay_policy_is_falling_behind(handle, 1, 2, 4) == 1)
    }

    @Test
    func aSenderWithoutAClockNeverTripsTheFreshnessGuard() {
        let handle = opc_relay_policy_create(0)
        defer { opc_relay_policy_destroy(handle) }
        opc_relay_policy_connected(handle, 0)
        #expect(opc_relay_policy_is_falling_behind(handle, 0, 0, 1) == 0)
        #expect(opc_relay_policy_is_falling_behind(handle, 0, 0, 100) == 0)
    }

    @Test
    func nullHandlesAreInert() {
        #expect(opc_relay_policy_tick(nil, 1) == OPC_RELAY_ACTION_NONE)
        #expect(opc_relay_policy_retry_count(nil) == 0)
        #expect(opc_relay_policy_is_falling_behind(nil, 1, 0, 10) == 0)
        opc_relay_policy_destroy(nil)
    }
}
