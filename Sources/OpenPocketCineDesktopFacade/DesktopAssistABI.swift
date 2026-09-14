import COpcDesktop
import Foundation
import OpenPocketViewCore

/// Assists the desktop shell asks the core for rather than working out itself: the
/// false-colour lattices, zebra thresholds on the feed's own axis, and the legend.
///
/// The Rust side owns no colour science. It passes the body's colour mode and ISO
/// through and gets back what the phones would paint.
func assistTransfer(colorMode: Int32, iso: Int32) -> MonitorTransfer {
    if (50...102_400).contains(Int(iso)) {
        ScopeExposureCeiling.setISO(Int(iso))
    }
    guard (0...255).contains(colorMode), let mode = ColorMode(rawValue: UInt8(colorMode))
    else { return .rec709 }
    return MonitorTransfer(mode)
}

/// The ordinals `OPC_FALSE_COLOR_*` in `opc_desktop_types.h`.
private func falseColorScale(_ ordinal: Int32) -> LiveFalseColorScale? {
    switch ordinal {
    case OPC_FALSE_COLOR_STOPS: .stops
    case OPC_FALSE_COLOR_IRE: .ire
    case OPC_FALSE_COLOR_LIMITS: .limits
    case OPC_FALSE_COLOR_EL_ZONE: .elZone
    default: nil
    }
}

/// One of the two false-colour lattices as a cube handle for `opc_lut_*`. `paint` is
/// non-zero for the zone colours, zero for the weight. Null for an unknown scale.
@_cdecl("opc_false_color_cube")
func opc_false_color_cube(_ scale: Int32, _ colorMode: Int32, _ iso: Int32, _ paint: Int32)
    -> UnsafeMutableRawPointer?
{
    guard let scale = falseColorScale(scale) else { return nil }
    let transfer = assistTransfer(colorMode: colorMode, iso: iso)
    let cube =
        paint != 0
        ? FalseColorCube.paint(scale: scale, transfer: transfer)
        : FalseColorCube.weight(scale: scale, transfer: transfer)
    return retainCube(cube)
}

/// Writes `[highlightNative, midtoneNative, midtoneHalfNative, peakingGateScale]`.
@_cdecl("opc_assist_scalars")
func opc_assist_scalars(
    _ colorMode: Int32, _ iso: Int32, _ highlightIRE: Float, _ midtoneIRE: Float,
    _ out: UnsafeMutablePointer<Float>?
) -> Int32 {
    guard let out else { return OPC_RELAY_ERR_NULL }
    let values = LiveAssistScalars.native(
        transfer: assistTransfer(colorMode: colorMode, iso: iso), iso: Int(iso),
        highlightIRE: Double(highlightIRE), midtoneIRE: Double(midtoneIRE))
    for (index, value) in values.enumerated() {
        out[index] = value
    }
    return OPC_RELAY_OK
}

/// The legend for a scale: one `label<TAB>red<TAB>green<TAB>blue` line per zone,
/// darkest first. Returns the byte count needed, like the other emitters.
@_cdecl("opc_false_color_legend")
func opc_false_color_legend(
    _ scale: Int32, _ colorMode: Int32, _ iso: Int32, _ out: UnsafeMutablePointer<UInt8>?,
    _ capacity: Int
) -> Int64 {
    guard let scale = falseColorScale(scale) else { return Int64(OPC_RELAY_ERR_NULL) }
    let transfer = assistTransfer(colorMode: colorMode, iso: iso)
    let lines = FalseColorCube.legend(scale: scale, transfer: transfer).map { band in
        "\(band.label)\t\(band.red)\t\(band.green)\t\(band.blue)"
    }
    return DesktopFacade.emit(Data(lines.joined(separator: "\n").utf8), into: out, capacity: capacity)
}
