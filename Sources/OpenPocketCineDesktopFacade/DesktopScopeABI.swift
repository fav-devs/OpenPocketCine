import COpcDesktop
import Foundation
import OpenPocketViewCore

/// The scopes' colour science for the desktop shell, which reads the picture on the
/// CPU and draws its own plates: where each code plots, where 18% grey sits, what the
/// traffic lights and the ND chip say about a histogram.

/// `ScopeDisplayScale.levelTable`: the plot level (0…1 of plot height) of each of the
/// 256 tap codes for this colour mode and ISO. Writes up to `capacity`; returns 256.
@_cdecl("opc_scope_level_table")
func opc_scope_level_table(
    _ colorMode: Int32, _ iso: Int32, _ out: UnsafeMutablePointer<Float>?, _ capacity: Int
) -> Int32 {
    let transfer = assistTransfer(colorMode: colorMode, iso: iso)
    let table = ScopeDisplayScale.levelTable(
        for: transfer, iso: (50...102_400).contains(Int(iso)) ? Int(iso) : nil)
    if let out {
        for (index, level) in table.prefix(capacity).enumerated() {
            out[index] = level
        }
    }
    return Int32(table.count)
}

/// 18% grey on the shared IRE scale, where WAVE draws its solid middle guide.
@_cdecl("opc_scope_grey_ire")
func opc_scope_grey_ire(_ colorMode: Int32, _ iso: Int32) -> Double {
    assistTransfer(colorMode: colorMode, iso: iso).scopeGreyScaleIRE
}

private func bins(_ pointer: UnsafePointer<Int32>?) -> [Int]? {
    guard let pointer else { return nil }
    return (0..<256).map { Int(pointer[$0]) }
}

private func light(_ values: UnsafePointer<Float>, at offset: Int) -> ScopeChannelLight {
    ScopeChannelLight(
        clip: values[offset] > 0.5, crush: values[offset + 1] > 0.5,
        level: Double(values[offset + 2]))
}

/// `ScopeTrafficLights.reading` from 256-bin native histograms. `previous` and `out`
/// are nine floats: clip, crush, level for red, green and blue. `luma` may be null.
@_cdecl("opc_scope_traffic_lights")
func opc_scope_traffic_lights(
    _ red: UnsafePointer<Int32>?, _ green: UnsafePointer<Int32>?, _ blue: UnsafePointer<Int32>?,
    _ luma: UnsafePointer<Int32>?, _ colorMode: Int32, _ iso: Int32, _ threshold: Double,
    _ previous: UnsafePointer<Float>?, _ out: UnsafeMutablePointer<Float>?
) -> Int32 {
    guard let red = bins(red), let green = bins(green), let blue = bins(blue), let out else {
        return OPC_RELAY_ERR_NULL
    }
    let transfer = assistTransfer(colorMode: colorMode, iso: iso)
    let last = previous.map {
        ScopeTrafficLightsReading(red: light($0, at: 0), green: light($0, at: 3), blue: light($0, at: 6))
    }
    let reading = ScopeTrafficLights.reading(
        red: red, green: green, blue: blue, luma: bins(luma), transfer: transfer,
        threshold: threshold, previous: last)
    for (index, channel) in [reading.red, reading.green, reading.blue].enumerated() {
        out[index * 3] = channel.clip ? 1 : 0
        out[index * 3 + 1] = channel.crush ? 1 : 0
        out[index * 3 + 2] = Float(channel.level)
    }
    return OPC_RELAY_OK
}

/// The ND chip's reading from the luma histogram, as
/// `pictureStops<TAB>ndStops<TAB>ndLabel<TAB>stopsLabel`. Empty when the picture gives
/// no reading.
@_cdecl("opc_scope_nd")
func opc_scope_nd(
    _ colorMode: Int32, _ iso: Int32, _ luma: UnsafePointer<Int32>?,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    guard let luma = bins(luma) else { return Int64(OPC_RELAY_ERR_NULL) }
    let transfer = assistTransfer(colorMode: colorMode, iso: iso)
    guard let reading = NDFilterRecommendation.reading(lumaHistogram: luma, transfer: transfer)
    else { return 0 }
    let text = "\(reading.pictureStops)\t\(reading.ndStops)\t\(reading.ndLabel)\t\(reading.stopsLabel)"
    return DesktopFacade.emit(Data(text.utf8), into: out, capacity: capacity)
}

/// The meters' floor in dBFS, where a silent bar sits.
@_cdecl("opc_audio_meter_floor_db")
func opc_audio_meter_floor_db() -> Double {
    AudioMeterBallistics.floorDB
}
