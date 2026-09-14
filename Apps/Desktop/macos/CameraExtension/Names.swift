import CoreVideo
import Foundation

/// The names the viewfinder's facade looks for (`DesktopVirtualCameraABI.swift`). Any
/// change here changes it there.
enum CameraNames {
    static let device = "OpenPocketCine"
    static let sourceStream = "OpenPocketCine"
    static let sinkStream = "OpenPocketCine Sink"
    static let model = "OpenPocketCine Virtual Camera"
    static let manufacturer = "OpenPocketCine"

    /// Fixed so apps remember the camera between launches.
    static let deviceID = UUID(uuidString: "5A1D7E9B-3C40-4F3B-9C2E-6F0B4C1E8D7A")!
    static let sourceStreamID = UUID(uuidString: "5A1D7E9B-3C40-4F3B-9C2E-6F0B4C1E8D7B")!
    static let sinkStreamID = UUID(uuidString: "5A1D7E9B-3C40-4F3B-9C2E-6F0B4C1E8D7C")!
}

/// What the picture is: 1280 × 720 NV12 (video range), 30 frames a second, as the
/// viewfinder sends it.
enum CameraFormat {
    static let width: Int32 = 1280
    static let height: Int32 = 720
    static let frameRate: Int32 = 30
    static let pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
}
