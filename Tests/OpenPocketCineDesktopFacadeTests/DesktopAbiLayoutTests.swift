import COpcDesktop
import Foundation
import Testing

/// The Rust host declares its own `#[repr(C)]` mirrors of these records. Its `layout`
/// tests assert the same numbers, so a field added on one side fails on both.
@Suite
struct DesktopAbiLayoutTests {
    @Test
    func messageHeaderLayoutIsStable() {
        #expect(MemoryLayout<OpcRelayMessageHeader>.size == 16)
        #expect(MemoryLayout<OpcRelayMessageHeader>.alignment == 4)
        #expect(MemoryLayout<OpcRelayMessageHeader>.offset(of: \.kind) == 0)
        #expect(MemoryLayout<OpcRelayMessageHeader>.offset(of: \.payload_offset) == 4)
        #expect(MemoryLayout<OpcRelayMessageHeader>.offset(of: \.payload_len) == 8)
        #expect(MemoryLayout<OpcRelayMessageHeader>.offset(of: \.consumed) == 12)
    }

    @Test
    func blobSplitAndFrameMetaLayoutIsStable() {
        #expect(MemoryLayout<OpcRelayBlobSplit>.size == 16)
        #expect(MemoryLayout<OpcRelayFrameMeta>.size == 32)
        #expect(MemoryLayout<OpcRelayFrameMeta>.alignment == 8)
        #expect(MemoryLayout<OpcRelayFrameMeta>.offset(of: \.encoded_at) == 24)
    }

    @Test
    func stateLayoutIsStable() {
        #expect(OPC_RELAY_TEXT_CAP == 64)
        #expect(OPC_RELAY_REASON_CAP == 256)
        #expect(OPC_RELAY_OPTIONS_CAP == 64)
        #expect(MemoryLayout<OpcRelayState>.size == 1312)
        #expect(MemoryLayout<OpcRelayState>.offset(of: \.iso_indices) == 32)
        #expect(MemoryLayout<OpcRelayState>.offset(of: \.format) == 800)
    }

    @Test
    func smallRecordLayoutIsStable() {
        #expect(MemoryLayout<OpcRelayJoinDenied>.size == 260)
        #expect(MemoryLayout<OpcRelayControlToken>.size == 68)
        #expect(MemoryLayout<OpcRelayHello>.size == 132)
        #expect(MemoryLayout<OpcRelayFocusPoint>.size == 8)
        #expect(MemoryLayout<OpcRelayProtocolInfo>.size == 112)
    }
}
