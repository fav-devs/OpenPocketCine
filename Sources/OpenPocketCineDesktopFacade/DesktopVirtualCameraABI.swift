import COpcDesktop
import Foundation
import OpenPocketViewCore

// MARK: - The viewfinder as a camera on macOS
//
// The camera itself is a CoreMediaIO camera extension (`Apps/Desktop/macos`), installed
// once from the OpenPocketCine Camera app. It offers a *source* stream every app reads and
// a *sink* stream the viewfinder writes to. This file is the writer: it finds the
// extension's device through CoreMediaIO, opens the sink stream's queue and enqueues
// NV12 sample buffers the Rust side hands over. Anywhere but macOS every entry point
// reports "unsupported".

#if canImport(CoreMediaIO)
    import CoreMedia
    import CoreMediaIO
    import CoreVideo

    /// The names the extension registers under; `Apps/Desktop/macos` uses the same.
    enum VirtualCameraNames {
        static let device = "OpenPocketCine"
        static let sink = "OpenPocketCine Sink"
    }

    private final class MacVirtualCamera {
        let device: CMIOObjectID
        let stream: CMIOStreamID
        let queue: CMSimpleQueue
        let format: CMFormatDescription
        let width: Int
        let height: Int

        init(
            device: CMIOObjectID, stream: CMIOStreamID, queue: CMSimpleQueue,
            format: CMFormatDescription, width: Int, height: Int
        ) {
            self.device = device
            self.stream = stream
            self.queue = queue
            self.format = format
            self.width = width
            self.height = height
        }

        deinit {
            CMIODeviceStopStream(device, stream)
        }
    }

    private final class VirtualCameraStore: @unchecked Sendable {
        private let lock = NSLock()
        private var camera: MacVirtualCamera?

        func replace(_ camera: MacVirtualCamera?) {
            lock.lock()
            defer { lock.unlock() }
            self.camera = camera
        }

        func withCamera<T>(_ body: (MacVirtualCamera?) -> T) -> T {
            lock.lock()
            defer { lock.unlock() }
            return body(camera)
        }
    }

    private let store = VirtualCameraStore()

    private func address(_ selector: CMIOObjectPropertySelector) -> CMIOObjectPropertyAddress {
        CMIOObjectPropertyAddress(
            mSelector: selector,
            mScope: CMIOObjectPropertyScope(kCMIOObjectPropertyScopeGlobal),
            mElement: CMIOObjectPropertyElement(kCMIOObjectPropertyElementMain))
    }

    /// An array property, or empty when the object does not have it.
    private func objectIDs(of object: CMIOObjectID, _ selector: CMIOObjectPropertySelector)
        -> [CMIOObjectID]
    {
        var addr = address(selector)
        var size: UInt32 = 0
        guard CMIOObjectGetPropertyDataSize(object, &addr, 0, nil, &size) == noErr, size > 0
        else { return [] }
        let count = Int(size) / MemoryLayout<CMIOObjectID>.size
        var ids = [CMIOObjectID](repeating: 0, count: count)
        var used: UInt32 = 0
        let status = ids.withUnsafeMutableBytes { bytes in
            CMIOObjectGetPropertyData(object, &addr, 0, nil, size, &used, bytes.baseAddress)
        }
        return status == noErr ? ids : []
    }

    /// `kCMIOObjectPropertyName`, or empty.
    private func name(of object: CMIOObjectID) -> String {
        var addr = address(CMIOObjectPropertySelector(kCMIOObjectPropertyName))
        var value: Unmanaged<CFString>?
        var used: UInt32 = 0
        let size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        let status = withUnsafeMutablePointer(to: &value) { pointer in
            CMIOObjectGetPropertyData(object, &addr, 0, nil, size, &used, pointer)
        }
        guard status == noErr, let value else { return "" }
        return value.takeRetainedValue() as String
    }

    /// The extension's device and its sink stream, found by name.
    private func findSink() -> (CMIOObjectID, CMIOStreamID)? {
        let system = CMIOObjectID(kCMIOObjectSystemObject)
        for device in objectIDs(of: system, CMIOObjectPropertySelector(kCMIOHardwarePropertyDevices))
        where name(of: device) == VirtualCameraNames.device {
            for stream in objectIDs(of: device, CMIOObjectPropertySelector(kCMIODevicePropertyStreams))
            where name(of: stream) == VirtualCameraNames.sink {
                return (device, stream)
            }
        }
        return nil
    }

    private func open(width: Int, height: Int) -> Int32 {
        guard let (device, stream) = findSink() else { return -1 }
        var queueOut: Unmanaged<CMSimpleQueue>?
        guard CMIOStreamCopyBufferQueue(stream, { _, _, _ in }, nil, &queueOut) == noErr,
            let queue = queueOut?.takeRetainedValue()
        else { return -2 }
        var format: CMFormatDescription?
        guard
            CMVideoFormatDescriptionCreate(
                allocator: kCFAllocatorDefault,
                codecType: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                width: Int32(width), height: Int32(height), extensions: nil,
                formatDescriptionOut: &format) == noErr, let format
        else { return -2 }
        guard CMIODeviceStartStream(device, stream) == noErr else { return -2 }
        store.replace(
            MacVirtualCamera(
                device: device, stream: stream, queue: queue, format: format, width: width,
                height: height))
        return 0
    }

    /// One NV12 frame into a pixel buffer, then a sample buffer timed now, onto the queue.
    private func push(_ camera: MacVirtualCamera, _ bytes: UnsafePointer<UInt8>, _ count: Int)
        -> Int32
    {
        let (w, h) = (camera.width, camera.height)
        let chromaRows = (h + 1) / 2
        guard count >= w * h + w * chromaRows else { return -3 }
        if CMSimpleQueueGetCount(camera.queue) >= CMSimpleQueueGetCapacity(camera.queue) {
            return 1  // The extension is behind; drop rather than block the viewfinder.
        }
        var pixelBuffer: CVPixelBuffer?
        let attributes: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary]
        guard
            CVPixelBufferCreate(
                kCFAllocatorDefault, w, h, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                attributes as CFDictionary, &pixelBuffer) == kCVReturnSuccess,
            let pixelBuffer
        else { return -3 }
        CVPixelBufferLockBaseAddress(pixelBuffer, [])
        if let luma = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 0) {
            let stride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 0)
            for row in 0..<h {
                memcpy(luma + row * stride, bytes + row * w, w)
            }
        }
        if let chroma = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, 1) {
            let stride = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, 1)
            let source = bytes + w * h
            for row in 0..<chromaRows {
                memcpy(chroma + row * stride, source + row * w, w)
            }
        }
        CVPixelBufferUnlockBaseAddress(pixelBuffer, [])

        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: 30),
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard
            CMSampleBufferCreateForImageBuffer(
                allocator: kCFAllocatorDefault, imageBuffer: pixelBuffer, dataReady: true,
                makeDataReadyCallback: nil, refcon: nil, formatDescription: camera.format,
                sampleTiming: &timing, sampleBufferOut: &sample) == noErr, let sample
        else { return -3 }
        // The queue takes the reference; CoreMediaIO releases it once consumed.
        let status = CMSimpleQueueEnqueue(camera.queue, element: Unmanaged.passRetained(sample).toOpaque())
        return status == noErr ? 0 : 1
    }

    /// Opens the extension's sink stream for `width × height` NV12 frames. 0 on
    /// success, -1 when the OpenPocketCine camera extension is not installed, -2 when its
    /// sink stream would not start.
    @_cdecl("opc_vcam_mac_open")
    func opc_vcam_mac_open(_ width: Int32, _ height: Int32) -> Int32 {
        guard width > 0, height > 0 else { return -3 }
        store.replace(nil)
        return open(width: Int(width), height: Int(height))
    }

    /// One NV12 frame. 0 delivered, 1 dropped because the extension is behind, negative
    /// for a malformed frame or no open camera.
    @_cdecl("opc_vcam_mac_push")
    func opc_vcam_mac_push(_ bytes: UnsafePointer<UInt8>?, _ count: Int) -> Int32 {
        guard let bytes, count > 0 else { return -3 }
        return store.withCamera { camera in
            guard let camera else { return -4 }
            return push(camera, bytes, count)
        }
    }

    /// Stops the sink stream and lets the device go.
    @_cdecl("opc_vcam_mac_close")
    func opc_vcam_mac_close() {
        store.replace(nil)
    }

    /// 1 when the camera extension's device and sink stream are there, else 0.
    @_cdecl("opc_vcam_mac_present")
    func opc_vcam_mac_present() -> Int32 {
        findSink() == nil ? 0 : 1
    }

#else

    @_cdecl("opc_vcam_mac_open")
    func opc_vcam_mac_open(_ width: Int32, _ height: Int32) -> Int32 { -5 }

    @_cdecl("opc_vcam_mac_push")
    func opc_vcam_mac_push(_ bytes: UnsafePointer<UInt8>?, _ count: Int) -> Int32 { -5 }

    @_cdecl("opc_vcam_mac_close")
    func opc_vcam_mac_close() {}

    @_cdecl("opc_vcam_mac_present")
    func opc_vcam_mac_present() -> Int32 { 0 }

#endif
