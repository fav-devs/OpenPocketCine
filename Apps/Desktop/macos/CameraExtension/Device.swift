import CoreMedia
import CoreMediaIO
import CoreVideo
import Foundation
import IOKit.audio

/// The device: a source stream apps read at 30 frames a second, fed from a sink stream
/// the viewfinder writes to. Black until the first frame arrives.
final class CameraDeviceSource: NSObject, CMIOExtensionDeviceSource {
    private(set) var device: CMIOExtensionDevice!
    private var sourceStream: CameraStreamSource!
    private var sinkStream: CameraSinkStreamSource!
    private var formatDescription: CMFormatDescription!
    private var bufferPool: CVPixelBufferPool!
    private let timerQueue = DispatchQueue(
        label: "com.opencapture.openpocketcine.camera.timer", qos: .userInteractive)
    private var timer: DispatchSourceTimer?
    private var readers = 0
    private var sinkClient: CMIOExtensionClient?
    private let lock = NSLock()
    private var latestImage: CVPixelBuffer?

    init(localizedName: String) {
        super.init()
        device = CMIOExtensionDevice(
            localizedName: localizedName, deviceID: CameraNames.deviceID, legacyDeviceID: nil,
            source: self)
        CMVideoFormatDescriptionCreate(
            allocator: kCFAllocatorDefault, codecType: CameraFormat.pixelFormat,
            width: CameraFormat.width, height: CameraFormat.height, extensions: nil,
            formatDescriptionOut: &formatDescription)
        let attributes: NSDictionary = [
            kCVPixelBufferWidthKey: CameraFormat.width,
            kCVPixelBufferHeightKey: CameraFormat.height,
            kCVPixelBufferPixelFormatTypeKey: CameraFormat.pixelFormat,
            kCVPixelBufferIOSurfacePropertiesKey: [:],
        ]
        CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, attributes, &bufferPool)
        let frameDuration = CMTime(value: 1, timescale: CameraFormat.frameRate)
        let format = CMIOExtensionStreamFormat(
            formatDescription: formatDescription, maxFrameDuration: frameDuration,
            minFrameDuration: frameDuration, validFrameDurations: nil)
        sourceStream = CameraStreamSource(
            localizedName: CameraNames.sourceStream, streamID: CameraNames.sourceStreamID,
            streamFormat: format, device: self)
        sinkStream = CameraSinkStreamSource(
            localizedName: CameraNames.sinkStream, streamID: CameraNames.sinkStreamID,
            streamFormat: format, device: self)
        do {
            try device.addStream(sourceStream.stream)
            try device.addStream(sinkStream.stream)
        } catch {
            fatalError("the streams could not be added: \(error)")
        }
    }

    var availableProperties: Set<CMIOExtensionProperty> {
        [.deviceTransportType, .deviceModel]
    }

    func deviceProperties(forProperties properties: Set<CMIOExtensionProperty>) throws
        -> CMIOExtensionDeviceProperties
    {
        let deviceProperties = CMIOExtensionDeviceProperties(dictionary: [:])
        if properties.contains(.deviceTransportType) {
            deviceProperties.transportType = kIOAudioDeviceTransportTypeVirtual
        }
        if properties.contains(.deviceModel) {
            deviceProperties.model = CameraNames.model
        }
        return deviceProperties
    }

    func setDeviceProperties(_ deviceProperties: CMIOExtensionDeviceProperties) throws {}

    // MARK: Source side: the apps

    func startStreaming() {
        readers += 1
        guard timer == nil else { return }
        let timer = DispatchSource.makeTimerSource(flags: .strict, queue: timerQueue)
        timer.schedule(
            deadline: .now(), repeating: 1.0 / Double(CameraFormat.frameRate),
            leeway: .milliseconds(2))
        timer.setEventHandler { [weak self] in self?.sendFrame() }
        timer.resume()
        self.timer = timer
    }

    func stopStreaming() {
        readers = max(0, readers - 1)
        if readers == 0 {
            timer?.cancel()
            timer = nil
        }
    }

    /// The newest frame, retimed to now, or black.
    private func sendFrame() {
        lock.lock()
        let image = latestImage
        lock.unlock()
        let pixelBuffer: CVPixelBuffer
        if let image {
            pixelBuffer = image
        } else if let black = blackFrame() {
            pixelBuffer = black
        } else {
            return
        }
        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: CameraFormat.frameRate),
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard
            CMSampleBufferCreateForImageBuffer(
                allocator: kCFAllocatorDefault, imageBuffer: pixelBuffer, dataReady: true,
                makeDataReadyCallback: nil, refcon: nil, formatDescription: formatDescription,
                sampleTiming: &timing, sampleBufferOut: &sample) == noErr, let sample
        else { return }
        sourceStream.stream.send(
            sample, discontinuity: [],
            hostTimeInNanoseconds: UInt64(timing.presentationTimeStamp.seconds * 1_000_000_000))
    }

    private var black: CVPixelBuffer?

    /// NV12 black: luma 16, chroma 128. Made once.
    private func blackFrame() -> CVPixelBuffer? {
        if let black { return black }
        var pixelBuffer: CVPixelBuffer?
        guard
            CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, bufferPool, &pixelBuffer)
                == kCVReturnSuccess, let pixelBuffer
        else { return nil }
        CVPixelBufferLockBaseAddress(pixelBuffer, [])
        if let luma = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 0) {
            memset(
                luma, 16,
                CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 0)
                    * CVPixelBufferGetHeightOfPlane(pixelBuffer, 0))
        }
        if let chroma = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 1) {
            memset(
                chroma, 128,
                CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 1)
                    * CVPixelBufferGetHeightOfPlane(pixelBuffer, 1))
        }
        CVPixelBufferUnlockBaseAddress(pixelBuffer, [])
        black = pixelBuffer
        return pixelBuffer
    }

    // MARK: Sink side: the viewfinder

    func startSinkStreaming(client: CMIOExtensionClient) {
        sinkClient = client
        consume(client)
    }

    func stopSinkStreaming() {
        sinkClient = nil
        lock.lock()
        latestImage = nil
        lock.unlock()
    }

    /// Pulls the next buffer the viewfinder enqueued, keeps its image, asks again.
    private func consume(_ client: CMIOExtensionClient) {
        guard sinkClient === client else { return }
        sinkStream.stream.consumeSampleBuffer(from: client) {
            [weak self] sample, sequenceNumber, _, _, error in
            guard let self, self.sinkClient === client else { return }
            if error == nil, let sample, let image = CMSampleBufferGetImageBuffer(sample) {
                self.lock.lock()
                self.latestImage = image
                self.lock.unlock()
                let output = CMIOExtensionScheduledOutput(
                    sequenceNumber: sequenceNumber,
                    hostTimeInNanoseconds: UInt64(
                        CMClockGetTime(CMClockGetHostTimeClock()).seconds * 1_000_000_000))
                if self.readers > 0 {
                    self.sinkStream.stream.notifyScheduledOutputChanged(output)
                }
            }
            self.consume(client)
        }
    }
}
