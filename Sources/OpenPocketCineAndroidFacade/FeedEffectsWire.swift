import Foundation
import OpenPocketViewCore

/// Bakes LUT-adjacent assist payloads for the Android GLES feed.
///
/// Overlay cubes and zebra codes follow iOS `PocketFalseColorMap` /
/// `LiveMonitorCompositor` so PEAK / FALSE / ZEBRA / LUT match the Metal path.
/// Compiles on Darwin so `swift test` exercises the same lattice as the APK.
public enum FeedEffectsWire {
    public static let falseColorCubeSize = FalseColorCube.size
    public static let assistScalarCount = 4

    public static func monitorTransfer(colorModeCode: Int) -> MonitorTransfer {
        guard (0...255).contains(colorModeCode),
            let mode = ColorMode(rawValue: UInt8(colorModeCode))
        else { return .rec709 }
        return MonitorTransfer(mode)
    }

    public static func falseColorScale(_ ordinal: Int) -> LiveFalseColorScale? {
        switch ordinal {
        case 0: .stops
        case 1: .ire
        case 2: .limits
        case 3: .elZone
        default: nil
        }
    }

    /// Packed-2D RGBA8 overlay paint (`n³ × 4`). Sampled on encoded camera codes.
    public static func packedFalseColorPaint(
        scaleOrdinal: Int, colorModeCode: Int, iso: Int
    ) -> [UInt8]? {
        guard let scale = falseColorScale(scaleOrdinal) else { return nil }
        let transfer = preparedTransfer(colorModeCode: colorModeCode, iso: iso)
        let key = cacheKey("paint", scale: scale, transfer: transfer)
        return cached(key) {
            LUTLibraryWire.packedRGBA(
                cube: FalseColorCube.paint(scale: scale, transfer: transfer))
        }
    }

    /// Packed-2D RGBA8 overlay weight. IRE / CineStop / EL Zone are opaque; Limits is holes-only.
    public static func packedFalseColorWeight(
        scaleOrdinal: Int, colorModeCode: Int, iso: Int
    ) -> [UInt8]? {
        guard let scale = falseColorScale(scaleOrdinal) else { return nil }
        let transfer = preparedTransfer(colorModeCode: colorModeCode, iso: iso)
        let key = cacheKey("weight", scale: scale, transfer: transfer)
        return cached(key) {
            LUTLibraryWire.packedRGBA(
                cube: FalseColorCube.weight(scale: scale, transfer: transfer))
        }
    }

    /// `[highlightNative, midtoneNative, midtoneHalfNative, peakingGateScale]`.
    public static func assistScalars(
        colorModeCode: Int, iso: Int, highlightIRE: Double, midtoneIRE: Double
    ) -> [Float] {
        let transfer = preparedTransfer(colorModeCode: colorModeCode, iso: iso)
        return LiveAssistScalars.native(
            transfer: transfer, iso: iso, highlightIRE: highlightIRE, midtoneIRE: midtoneIRE)
    }

    /// Display-referred feeds read larger gradients than log (iOS `peakingGateScale`).
    public static func peakingGateScale(for transfer: MonitorTransfer) -> Double {
        LiveAssistScalars.peakingGateScale(for: transfer)
    }

    private static func preparedTransfer(colorModeCode: Int, iso: Int) -> MonitorTransfer {
        if (50...102_400).contains(iso) {
            ScopeExposureCeiling.setISO(iso)
        }
        return monitorTransfer(colorModeCode: colorModeCode)
    }

    private static let cacheLock = NSLock()
    // Protected by cacheLock.
    nonisolated(unsafe) private static var cubeCache: [String: [UInt8]] = [:]

    private static func cacheKey(
        _ kind: String, scale: LiveFalseColorScale, transfer: MonitorTransfer
    ) -> String {
        let clip = ScopeExposureCeiling.clipByte(transfer: transfer)
        return "\(kind):\(scale.rawValue):\(transfer.rawValue):\(clip)"
    }

    private static func cached(_ key: String, build: () -> [UInt8]) -> [UInt8] {
        cacheLock.lock()
        if let hit = cubeCache[key] {
            cacheLock.unlock()
            return hit
        }
        cacheLock.unlock()
        let built = build()
        cacheLock.lock()
        cubeCache[key] = built
        cacheLock.unlock()
        return built
    }
}
