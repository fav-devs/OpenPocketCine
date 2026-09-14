import Foundation

/// The false-colour overlay as a colour cube, so every shell paints the same zones.
///
/// Each shell samples the ungraded feed through two 64³ lattices: `paint` is the zone
/// colour for that code, `weight` how much of it shows (1 for CineStop / IRE / EL Zone,
/// which repaint the whole picture; a hole-only mask for Limits). Moved here from the
/// Android facade so the desktop grade reads the same lattice instead of a copy.
public enum FalseColorCube {
    public static let size = 64

    /// Zone colours, sampled on encoded camera codes.
    public static func paint(scale: LiveFalseColorScale, transfer: MonitorTransfer) -> CubeLUT {
        cube(scale: scale, transfer: transfer) { ($0.red, $0.green, $0.blue) }
    }

    /// How much of the paint shows. IRE / CineStop / EL Zone are opaque; Limits is holes-only.
    public static func weight(scale: LiveFalseColorScale, transfer: MonitorTransfer) -> CubeLUT {
        cube(scale: scale, transfer: transfer) { ($0.weight, $0.weight, $0.weight) }
    }

    /// The zones a legend shows for `scale`, darkest first.
    public static func legend(scale: LiveFalseColorScale, transfer: MonitorTransfer)
        -> [LiveFalseColorBand]
    {
        LiveColorScience.falseColorBands(scale, transfer: transfer)
    }

    public static func cube(
        scale: LiveFalseColorScale,
        transfer: MonitorTransfer,
        component: ((red: Double, green: Double, blue: Double, weight: Double)) -> (
            Double, Double, Double
        )
    ) -> CubeLUT {
        let size = Self.size
        let denom = Double(size - 1)
        let bandList = LiveColorScience.falseColorBands(scale, transfer: transfer)
        var rgb = [Float]()
        rgb.reserveCapacity(size * size * size * 3)
        for b in 0..<size {
            for g in 0..<size {
                for r in 0..<size {
                    let er = Double(r) / denom
                    let eg = Double(g) / denom
                    let eb = Double(b) / denom
                    let yEnc = encodedLuma(red: er, green: eg, blue: eb, transfer: transfer)
                    let ire = ScopeDisplayScale.monitorPercent(yEnc, transfer: transfer)
                    let value =
                        scale.usesSceneStops
                        ? LiveColorScience.stops(encoded: yEnc, transfer: transfer) : ire
                    let chosen = component(
                        overlayPaint(
                            value: value, scale: scale, bands: bandList,
                            monitorGray: ire / 100))
                    rgb.append(Float(chosen.0))
                    rgb.append(Float(chosen.1))
                    rgb.append(Float(chosen.2))
                }
            }
        }
        return CubeLUT(size: size, rgb: rgb)
    }

    private static func encodedLuma(
        red: Double, green: Double, blue: Double, transfer: MonitorTransfer
    ) -> Double {
        let w = LiveColorScience.lumaWeights(transfer)
        return w.red * red + w.green * green + w.blue * blue
    }

    private static func overlayPaint(
        value: Double, scale: LiveFalseColorScale, bands: [LiveFalseColorBand],
        monitorGray: Double
    ) -> (red: Double, green: Double, blue: Double, weight: Double) {
        switch scale {
        case .stops, .ire, .elZone:
            let color = renderedColor(
                value: value, scale: scale, bands: bands,
                source: (0, 0, 0), monitorGray: monitorGray)
            return (color.red, color.green, color.blue, 1)
        case .limits:
            break
        }
        let width = transitionWidth(scale)
        var paint = (red: 0.0, green: 0.0, blue: 0.0)
        var total = 0.0
        for item in bands {
            let weight = bandWeight(value: value, band: item, width: width)
            paint.red += item.red * weight
            paint.green += item.green * weight
            paint.blue += item.blue * weight
            total += weight
        }
        guard total > 0 else { return (0, 0, 0, 0) }
        return (paint.red / total, paint.green / total, paint.blue / total, min(1, total))
    }

    private static func renderedColor(
        value: Double,
        scale: LiveFalseColorScale,
        bands: [LiveFalseColorBand],
        source: (red: Double, green: Double, blue: Double),
        monitorGray: Double
    ) -> (red: Double, green: Double, blue: Double) {
        let base: (red: Double, green: Double, blue: Double)
        switch scale {
        case .stops, .ire, .elZone:
            let gray = min(1, max(0, monitorGray))
            base = (gray, gray, gray)
        case .limits:
            base = (
                min(1, max(0, source.red)),
                min(1, max(0, source.green)),
                min(1, max(0, source.blue))
            )
        }
        let weighted = bands.map {
            ($0, bandWeight(value: value, band: $0, width: transitionWidth(scale)))
        }
        let total = weighted.reduce(0) { $0 + $1.1 }
        guard total > 0 else { return base }
        let normalization = max(1, total)
        let baseWeight = max(0, 1 - total)
        let painted = weighted.reduce(
            (
                red: base.red * baseWeight, green: base.green * baseWeight,
                blue: base.blue * baseWeight
            )
        ) { result, item in
            (
                result.red + item.0.red * item.1,
                result.green + item.0.green * item.1,
                result.blue + item.0.blue * item.1
            )
        }
        return (
            painted.red / normalization,
            painted.green / normalization,
            painted.blue / normalization
        )
    }

    private static func bandWeight(
        value: Double, band: LiveFalseColorBand, width: Double
    ) -> Double {
        let rising =
            band.lowerBound.isFinite && band.lowerBound != 0
            ? smoothStep(
                edge0: band.lowerBound - width, edge1: band.lowerBound + width, value: value)
            : 1
        let falling =
            band.upperBound.isFinite
            ? 1
                - smoothStep(
                    edge0: band.upperBound - width, edge1: band.upperBound + width, value: value)
            : 1
        return rising * falling
    }

    private static func smoothStep(edge0: Double, edge1: Double, value: Double) -> Double {
        let span = edge1 - edge0
        guard span != 0 else { return value >= edge1 ? 1 : 0 }
        let progress = min(1, max(0, (value - edge0) / span))
        return progress * progress * (3 - 2 * progress)
    }

    private static func transitionWidth(_ scale: LiveFalseColorScale) -> Double {
        switch scale {
        case .elZone: 0.05
        case .stops, .ire, .limits: 0.5
        }
    }
}

/// Zebra thresholds and the peaking gate on the feed's own axis, per colour mode.
///
/// The operator sets zebra in IRE; the shader compares camera codes. This is the one
/// place that conversion happens, so a phone and a PC stripe the same pixels.
public enum LiveAssistScalars {
    /// `[highlightNative, midtoneNative, midtoneHalfNative, peakingGateScale]`.
    public static func native(
        transfer: MonitorTransfer, iso: Int, highlightIRE: Double, midtoneIRE: Double
    ) -> [Float] {
        let highlight = ScopeDisplayScale.signalNative(
            monitorPercent: highlightIRE, transfer: transfer, iso: iso)
        let half = LiveZebra.midtoneHalfWidthIRE
        let lo = ScopeDisplayScale.signalNative(
            monitorPercent: midtoneIRE - half, transfer: transfer, iso: iso)
        let hi = ScopeDisplayScale.signalNative(
            monitorPercent: midtoneIRE + half, transfer: transfer, iso: iso)
        return [
            Float(highlight),
            Float((lo + hi) * 0.5),
            Float(abs(hi - lo) * 0.5),
            Float(peakingGateScale(for: transfer)),
        ]
    }

    /// Display-referred feeds read larger gradients than log (iOS `peakingGateScale`).
    public static func peakingGateScale(for transfer: MonitorTransfer) -> Double {
        switch transfer {
        case .rec709, .hdr:
            let gradient = 1.57
            return gradient * gradient
        case .dlog, .dlog2, .dlogm:
            return 1
        }
    }
}
