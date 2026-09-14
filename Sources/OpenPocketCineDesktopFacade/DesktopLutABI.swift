import COpcDesktop
import Foundation
import OpenPocketViewCore

/// `.cube` parsing for the desktop shell.
///
/// The renderer needs a 3D texture, not a parser. Everything about reading a cube —
/// the size range, the sample-count check, Resolve's 65³ resample down to 64³ — stays
/// in `CubeLUT` so the desktop grade matches what the phones show.
final class CubeBox {
    let lut: CubeLUT

    init(_ lut: CubeLUT) {
        self.lut = lut
    }
}

private func cube(_ handle: UnsafeMutableRawPointer?) -> CubeLUT? {
    guard let handle else { return nil }
    return Unmanaged<CubeBox>.fromOpaque(handle).takeUnretainedValue().lut
}

func retainCube(_ lut: CubeLUT) -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(CubeBox(lut)).toOpaque()
}

/// Parses `.cube` text. Returns null and writes a message into `error` on failure.
@_cdecl("opc_lut_parse")
func opc_lut_parse(
    _ text: UnsafePointer<UInt8>?, _ count: Int, _ error: UnsafeMutablePointer<UInt8>?,
    _ errorCapacity: Int
) -> UnsafeMutableRawPointer? {
    func report(_ message: String) {
        guard let error, errorCapacity > 0 else { return }
        DesktopFacade.writeText(
            message, into: UnsafeMutableRawPointer(error), capacity: errorCapacity)
    }
    guard let text, count >= 0 else {
        report("No cube text was supplied.")
        return nil
    }
    let data = DesktopFacade.borrow(text, count)
    guard let source = String(data: data, encoding: .utf8) else {
        report("That .cube file is not UTF-8 text.")
        return nil
    }
    do {
        return retainCube(try CubeLUT.parse(source).colorCube)
    } catch let failure as CubeLUTParseError {
        report(failure.errorDescription ?? "That .cube file could not be read.")
        return nil
    } catch {
        report("That .cube file could not be read.")
        return nil
    }
}

/// One of the core's built-in looks, by name. Null when the name is not one of them.
@_cdecl("opc_lut_builtin")
func opc_lut_builtin(_ name: UnsafePointer<CChar>?, _ size: Int32) -> UnsafeMutableRawPointer? {
    guard let name, let look = BuiltInLook(rawValue: String(cString: name)) else { return nil }
    return retainCube(look.cube(size: Int(size)))
}

@_cdecl("opc_lut_destroy")
func opc_lut_destroy(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    Unmanaged<CubeBox>.fromOpaque(handle).release()
}

/// Lattice edge length. A texture upload wants `size³` RGBA texels.
@_cdecl("opc_lut_size")
func opc_lut_size(_ handle: UnsafeMutableRawPointer?) -> Int32 {
    Int32(cube(handle)?.size ?? 0)
}

/// Writes `size³ × 4` floats, red-fastest with alpha 1 — the layout a 3D texture wants.
@_cdecl("opc_lut_rgba")
func opc_lut_rgba(
    _ handle: UnsafeMutableRawPointer?, _ out: UnsafeMutablePointer<Float>?, _ capacity: Int
) -> Int64 {
    guard let lut = cube(handle) else { return Int64(OPC_RELAY_ERR_NULL) }
    let components = lut.rgbaComponents
    if let out, capacity >= components.count {
        components.withUnsafeBufferPointer { source in
            guard let base = source.baseAddress else { return }
            out.update(from: base, count: components.count)
        }
    }
    return Int64(components.count)
}

/// Trilinear resample onto an `n³` lattice, so a host can match the GPU's texture size.
@_cdecl("opc_lut_resampled")
func opc_lut_resampled(_ handle: UnsafeMutableRawPointer?, _ target: Int32)
    -> UnsafeMutableRawPointer?
{
    guard let lut = cube(handle) else { return nil }
    return retainCube(lut.resampled(to: Int(target)))
}

/// CPU reference for one sample, so a renderer can be checked against the core.
@_cdecl("opc_lut_map")
func opc_lut_map(
    _ handle: UnsafeMutableRawPointer?, _ red: Float, _ green: Float, _ blue: Float,
    _ out: UnsafeMutablePointer<Float>?
) -> Int32 {
    guard let lut = cube(handle), let out else { return OPC_RELAY_ERR_NULL }
    let mapped = lut.map(red: red, green: green, blue: blue)
    out[0] = mapped.red
    out[1] = mapped.green
    out[2] = mapped.blue
    return OPC_RELAY_OK
}

/// Comma-separated built-in look names, so the shell does not hardcode the list.
@_cdecl("opc_lut_builtin_names")
func opc_lut_builtin_names(_ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int) -> Int64 {
    let names = BuiltInLook.allCases.map(\.rawValue).joined(separator: ",")
    return DesktopFacade.emit(Data(names.utf8), into: out, capacity: capacity)
}
