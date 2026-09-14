import Foundation
import Testing

@testable import OpenPocketViewCore

/// The lattice every shell samples false colour through.
@Suite
struct FalseColorCubeTests {
    @Test
    func opaqueScalesPaintEverything() {
        let weight = FalseColorCube.weight(scale: .stops, transfer: .rec709)
        #expect(weight.size == FalseColorCube.size)
        #expect(weight.rgb.allSatisfy { $0 == 1 })
    }

    @Test
    func limitsLeavesTheMiddleAlone() {
        let weight = FalseColorCube.weight(scale: .limits, transfer: .rec709)
        let size = weight.size
        let mid = size / 2
        let index = (mid + mid * size + mid * size * size) * 3
        #expect(weight.rgb[index] == 0)
        #expect(weight.rgb[0] == 1)
    }

    @Test
    func theZebraAxisFollowsTheTransfer() {
        let log = LiveAssistScalars.native(
            transfer: .dlog, iso: 400, highlightIRE: 99, midtoneIRE: 55)
        let video = LiveAssistScalars.native(
            transfer: .rec709, iso: 400, highlightIRE: 99, midtoneIRE: 55)
        #expect(log.count == 4)
        #expect(log[0] != video[0])
        #expect(log[3] == 1)
        #expect(video[3] > 1)
    }
}
