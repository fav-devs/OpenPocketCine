import COpcDesktop
import Foundation
import OpenPocketViewCore
import Testing

@testable import OpenPocketCineDesktopFacade

/// The desktop shell paints false colour and zebra from what the core hands it, so
/// these check the hand-over rather than the colour science (that has its own tests).
@Suite
struct DesktopAssistAbiTests {
    @Test
    func theFalseColourLatticesComeBackAsCubes() throws {
        let paint = try #require(
            opc_false_color_cube(OPC_FALSE_COLOR_STOPS, Int32(ColorMode.normal.rawValue), 400, 1))
        defer { opc_lut_destroy(paint) }
        let weight = try #require(
            opc_false_color_cube(OPC_FALSE_COLOR_IRE, Int32(ColorMode.dLog.rawValue), 400, 0))
        defer { opc_lut_destroy(weight) }
        #expect(opc_lut_size(paint) == Int32(FalseColorCube.size))
        #expect(opc_lut_size(weight) == Int32(FalseColorCube.size))
        #expect(opc_false_color_cube(9, 0x3F, 400, 1) == nil)
    }

    @Test
    func zebraThresholdsLandOnTheFeedAxis() {
        var out = [Float](repeating: 0, count: 4)
        #expect(opc_assist_scalars(Int32(ColorMode.normal.rawValue), 400, 99, 55, &out) == OPC_RELAY_OK)
        #expect(out[0] > out[1])
        #expect(out[2] > 0)
        #expect(out[3] > 1)
        #expect(opc_assist_scalars(0x3F, 400, 99, 55, nil) == OPC_RELAY_ERR_NULL)
    }

    @Test
    func theLegendListsEveryZoneOnce() throws {
        let needed = opc_false_color_legend(OPC_FALSE_COLOR_EL_ZONE, 0x3F, 400, nil, 0)
        #expect(needed > 0)
        var bytes = [UInt8](repeating: 0, count: Int(needed))
        #expect(opc_false_color_legend(OPC_FALSE_COLOR_EL_ZONE, 0x3F, 400, &bytes, bytes.count) == needed)
        let text = try #require(String(bytes: bytes, encoding: .utf8))
        let lines = text.split(separator: "\n")
        #expect(lines.count == FalseColorCube.legend(scale: .elZone, transfer: .rec709).count)
        #expect(lines.allSatisfy { $0.split(separator: "\t").count == 4 })
    }
}
