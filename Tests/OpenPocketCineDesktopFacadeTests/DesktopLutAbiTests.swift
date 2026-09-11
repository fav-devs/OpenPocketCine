import COpcDesktop
import Foundation
import OpenPocketViewCore
import Testing

@testable import OpenPocketCineDesktopFacade

/// The desktop shell must not grow a second `.cube` parser. These cover the entry
/// points it reaches the core's through.
@Suite
struct DesktopLutAbiTests {
    private let identity = """
        TITLE "identity"
        LUT_3D_SIZE 2
        0.0 0.0 0.0
        1.0 0.0 0.0
        0.0 1.0 0.0
        1.0 1.0 0.0
        0.0 0.0 1.0
        1.0 0.0 1.0
        0.0 1.0 1.0
        1.0 1.0 1.0
        """

    private func parse(_ text: String) -> UnsafeMutableRawPointer? {
        var bytes = [UInt8](text.utf8)
        var message = [UInt8](repeating: 0, count: 256)
        return opc_lut_parse(&bytes, bytes.count, &message, message.count)
    }

    @Test
    func anIdentityCubeParsesAndRoundTrips() throws {
        let handle = try #require(parse(identity))
        defer { opc_lut_destroy(handle) }
        #expect(opc_lut_size(handle) == 2)

        let needed = opc_lut_rgba(handle, nil, 0)
        #expect(needed == 2 * 2 * 2 * 4)
        var rgba = [Float](repeating: 0, count: Int(needed))
        #expect(opc_lut_rgba(handle, &rgba, rgba.count) == needed)
        // Red-fastest with an opaque alpha is what a 3D texture upload expects.
        #expect(rgba[0] == 0)
        #expect(rgba[3] == 1)
        #expect(rgba[4] == 1)

        var mapped = [Float](repeating: 0, count: 3)
        #expect(opc_lut_map(handle, 0.25, 0.5, 0.75, &mapped) == OPC_RELAY_OK)
        let reference = try CubeLUT.parse(identity).colorCube.map(red: 0.25, green: 0.5, blue: 0.75)
        #expect(mapped[0] == reference.red)
        #expect(mapped[1] == reference.green)
        #expect(mapped[2] == reference.blue)
    }

    @Test
    func aBadCubeReportsTheCoreMessageRatherThanCrashing() {
        var bytes = [UInt8]("LUT_3D_SIZE 2\n0.0 0.0 0.0\n".utf8)
        var message = [UInt8](repeating: 0, count: 256)
        let handle = opc_lut_parse(&bytes, bytes.count, &message, message.count)
        #expect(handle == nil)
        let text = String(cString: message.map { CChar(bitPattern: $0) })
        #expect(!text.isEmpty)
    }

    @Test
    func builtInLooksComeFromTheCoreList() throws {
        let needed = opc_lut_builtin_names(nil, 0)
        var bytes = [UInt8](repeating: 0, count: Int(needed))
        #expect(opc_lut_builtin_names(&bytes, bytes.count) == needed)
        let names = String(decoding: bytes, as: UTF8.self).split(separator: ",").map(String.init)
        #expect(names == BuiltInLook.allCases.map(\.rawValue))

        let handle = try #require("Mono".withCString { opc_lut_builtin($0, 17) })
        defer { opc_lut_destroy(handle) }
        #expect(opc_lut_size(handle) == 17)
        var mapped = [Float](repeating: 0, count: 3)
        #expect(opc_lut_map(handle, 0.8, 0.2, 0.4, &mapped) == OPC_RELAY_OK)
        // Mono collapses the channels, so a graded sample is grey.
        #expect(abs(mapped[0] - mapped[1]) < 0.02)
        #expect(abs(mapped[1] - mapped[2]) < 0.02)
    }

    @Test
    func anUnknownBuiltInNameIsRefused() {
        #expect("Nope".withCString { opc_lut_builtin($0, 17) } == nil)
    }

    @Test
    func resamplingMatchesTheCoreLattice() throws {
        let handle = try #require(parse(identity))
        defer { opc_lut_destroy(handle) }
        let resampled = try #require(opc_lut_resampled(handle, 8))
        defer { opc_lut_destroy(resampled) }
        #expect(opc_lut_size(resampled) == 8)
        #expect(opc_lut_rgba(resampled, nil, 0) == 8 * 8 * 8 * 4)
    }

    @Test
    func nullHandlesAreInert() {
        #expect(opc_lut_size(nil) == 0)
        #expect(opc_lut_rgba(nil, nil, 0) == Int64(OPC_RELAY_ERR_NULL))
        #expect(opc_lut_resampled(nil, 8) == nil)
        opc_lut_destroy(nil)
    }
}
