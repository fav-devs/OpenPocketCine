import COpcDesktop
import Foundation
import OpenPocketViewCore
import Testing

@testable import OpenPocketCineDesktopFacade

/// Exercises the exported C entry points the way the Rust host calls them.
@Suite
struct DesktopRelayAbiTests {
    private func encode(kind: Int32, payload: Data) -> [UInt8] {
        var out = [UInt8](repeating: 0, count: payload.count + 16)
        let written = payload.withUnsafeBytes { raw -> Int64 in
            let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self)
            return opc_relay_framing_encode(
                UInt8(kind), base, payload.count, &out, out.count)
        }
        #expect(written > 0)
        return Array(out.prefix(Int(written)))
    }

    @Test
    func framingRoundTripReportsPayloadOffsets() {
        let payload = Data([0x0A, 0x0B, 0x0C, 0x0D])
        let wire = encode(kind: OPC_RELAY_KIND_STATE, payload: payload)
        #expect(wire.count == 9)

        var header = OpcRelayMessageHeader()
        var buffer = wire
        let status = opc_relay_framing_decode(&buffer, buffer.count, &header)
        #expect(status == OPC_RELAY_OK)
        #expect(header.kind == UInt8(OPC_RELAY_KIND_STATE))
        #expect(header.consumed == 9)
        #expect(header.payload_offset == 5)
        #expect(header.payload_len == 4)
        let slice = buffer[Int(header.payload_offset)..<Int(header.consumed)]
        #expect(Array(slice) == Array(payload))
    }

    @Test
    func framingWaitsForTheRestOfAMessage() {
        let wire = encode(kind: OPC_RELAY_KIND_STATE, payload: Data([1, 2, 3, 4]))
        var header = OpcRelayMessageHeader()
        for truncated in 0..<wire.count {
            var partial = Array(wire.prefix(truncated))
            let status = opc_relay_framing_decode(&partial, partial.count, &header)
            #expect(status == OPC_RELAY_NEED_MORE)
        }
    }

    @Test
    func framingRejectsAnUnknownKind() {
        var wire: [UInt8] = [0, 0, 0, 1, 0x7F]
        var header = OpcRelayMessageHeader()
        #expect(opc_relay_framing_decode(&wire, wire.count, &header) == OPC_RELAY_ERR_UNKNOWN_KIND)
    }

    @Test
    func framingEncodeReportsTheSizeItNeeds() {
        let payload = Data([1, 2, 3, 4])
        let needed = payload.withUnsafeBytes { raw -> Int64 in
            let base = raw.baseAddress?.assumingMemoryBound(to: UInt8.self)
            return opc_relay_framing_encode(
                UInt8(OPC_RELAY_KIND_STATE), base, payload.count, nil, 0)
        }
        #expect(needed == 9)
    }

    @Test
    func frameBlobSplitsMetadataFromTheAccessUnit() throws {
        let parameterSet = Data([0x00, 0x00, 0x00, 0x01, 0x40])
        let hevc = Data([0x09, 0x08, 0x07, 0x06])
        let metadata = WatcherRelayFrameMetadata(
            isKeyframe: true, parameterSets: [parameterSet], isRecording: true,
            extraMirrored: true, encodedAt: 12.5)
        var blob = [UInt8](try WatcherRelayFrameBlob.encode(metadata: metadata, hevc: hevc))

        var split = OpcRelayBlobSplit()
        var meta = OpcRelayFrameMeta()
        #expect(opc_relay_frame_blob_decode(&blob, blob.count, &split, &meta) == OPC_RELAY_OK)
        #expect(split.meta_offset == 4)
        #expect(Int(split.hevc_offset) == blob.count - hevc.count)
        #expect(split.hevc_len == UInt32(hevc.count))
        let carved = blob[Int(split.hevc_offset)..<(Int(split.hevc_offset) + Int(split.hevc_len))]
        #expect(Array(carved) == Array(hevc))
        #expect(meta.is_keyframe == 1)
        #expect(meta.is_recording == 1)
        #expect(meta.extra_mirrored == 1)
        #expect(meta.has_encoded_at == 1)
        #expect(meta.encoded_at == 12.5)
        #expect(meta.parameter_set_count == 1)
        #expect(meta.codec == Int32(WatcherRelayProtocol.hevcCodec))

        let metaStart = Int(split.meta_offset)
        var metaJSON = Array(blob[metaStart..<(metaStart + Int(split.meta_len))])
        var set = [UInt8](repeating: 0, count: 32)
        let written = opc_relay_frame_parameter_set(
            &metaJSON, metaJSON.count, 0, &set, set.count)
        #expect(written == Int64(parameterSet.count))
        #expect(Array(set.prefix(Int(written))) == Array(parameterSet))
        #expect(
            opc_relay_frame_parameter_set(&metaJSON, metaJSON.count, 1, &set, set.count)
                == Int64(OPC_RELAY_ERR_OUT_OF_RANGE))
    }

    @Test
    func frameMetadataWithoutAClockIsFlaggedNotDefaulted() throws {
        let metadata = WatcherRelayFrameMetadata(isKeyframe: false)
        var blob = [UInt8](try WatcherRelayFrameBlob.encode(metadata: metadata, hevc: Data([1])))
        var split = OpcRelayBlobSplit()
        var meta = OpcRelayFrameMeta()
        #expect(opc_relay_frame_blob_decode(&blob, blob.count, &split, &meta) == OPC_RELAY_OK)
        #expect(meta.has_encoded_at == 0)
        #expect(meta.parameter_set_count == 0)
    }

    @Test
    func stateDecodeCarriesLabelsAndControlOptions() throws {
        let state = WatcherRelayState(
            isRecording: true, format: "3840×2160 · 30p", color: "D-Log M", zoom: "1.0×",
            liveFPS: "25", batteryPercent: 73, cameraName: "Osmo Pocket 4 Pro", iso: "400",
            shutter: "1/50", allowsControlRequests: true,
            controlOptions: WatcherRelayControlOptions(
                isoIndices: [100, 200, 400], shutterDenominators: [50, 60],
                zoomHundredths: [100, 200]),
            cameraModel: "Pocket 4 Pro", isNano: false)
        var json = [UInt8](try JSONEncoder().encode(state))
        var out = OpcRelayState()
        #expect(opc_relay_state_decode(&json, json.count, &out) == OPC_RELAY_OK)
        #expect(out.is_recording == 1)
        #expect(out.battery_percent == 73)
        #expect(out.allows_control_requests == 1)
        #expect(out.is_nano == 0)
        #expect(out.has_control_options == 1)
        #expect(out.iso_count == 3)
        #expect(out.shutter_count == 2)
        #expect(out.zoom_count == 2)
        #expect(text(&out.camera_name) == "Osmo Pocket 4 Pro")
        #expect(text(&out.format) == "3840×2160 · 30p")
        #expect(text(&out.shutter) == "1/50")
        #expect(text(&out.camera_model) == "Pocket 4 Pro")
    }

    @Test
    func stateDecodeMarksAnUnstatedNanoFlagAsUnknown() throws {
        var json = [UInt8](try JSONEncoder().encode(WatcherRelayState()))
        var out = OpcRelayState()
        #expect(opc_relay_state_decode(&json, json.count, &out) == OPC_RELAY_OK)
        #expect(out.is_nano == -1)
        #expect(out.has_control_options == 0)
        #expect(out.iso_count == 0)
    }

    @Test
    func stateDecodeClampsOversizedOptionListsAndLongLabels() throws {
        let cap = Int(OPC_RELAY_OPTIONS_CAP)
        let state = WatcherRelayState(
            cameraName: String(repeating: "n", count: 200),
            controlOptions: WatcherRelayControlOptions(
                isoIndices: Array(0..<(cap + 10)), shutterDenominators: [], zoomHundredths: []))
        var json = [UInt8](try JSONEncoder().encode(state))
        var out = OpcRelayState()
        #expect(opc_relay_state_decode(&json, json.count, &out) == OPC_RELAY_OK)
        #expect(out.iso_count == Int32(cap))
        #expect(text(&out.camera_name).count == Int(OPC_RELAY_TEXT_CAP) - 1)
    }

    @Test
    func joinDeniedAndControlTokenDecode() throws {
        var denied = [UInt8](
            try JSONEncoder().encode(
                WatcherRelayJoinDenied(reason: "Wrong passcode.", passcodeRequired: true)))
        var deniedOut = OpcRelayJoinDenied()
        #expect(opc_relay_join_denied_decode(&denied, denied.count, &deniedOut) == OPC_RELAY_OK)
        #expect(deniedOut.passcode_required == 1)
        #expect(text(&deniedOut.reason) == "Wrong passcode.")

        var token = [UInt8](
            try JSONEncoder().encode(
                WatcherRelayControlToken(holderName: "Studio PC", holderIsRecipient: true)))
        var tokenOut = OpcRelayControlToken()
        #expect(opc_relay_control_token_decode(&token, token.count, &tokenOut) == OPC_RELAY_OK)
        #expect(tokenOut.holder_is_recipient == 1)
        #expect(text(&tokenOut.holder_name) == "Studio PC")
    }

    @Test
    func helloEncodeOmitsAnEmptyPasscodeAndDecodesBack() throws {
        var buffer = [UInt8](repeating: 0, count: 512)
        let written = "Studio PC".withCString { host in
            "".withCString { passcode in
                "watcher-1".withCString { identifier in
                    opc_relay_hello_encode(
                        host, passcode, identifier, &buffer, buffer.count)
                }
            }
        }
        #expect(written > 0)
        let payload = Data(buffer.prefix(Int(written)))
        let hello = try JSONDecoder().decode(WatcherRelayHello.self, from: payload)
        #expect(hello.hostName == "Studio PC")
        #expect(hello.passcode == nil)
        #expect(hello.watcherID == "watcher-1")
        #expect(hello.version == WatcherRelayProtocol.version)

        var helloBytes = [UInt8](payload)
        var out = OpcRelayHello()
        #expect(opc_relay_hello_decode(&helloBytes, helloBytes.count, &out) == OPC_RELAY_OK)
        #expect(out.version == Int32(WatcherRelayProtocol.version))
        #expect(text(&out.host_name) == "Studio PC")
    }

    @Test
    func commandEncodeMatchesTheCoreEnum() throws {
        func roundTrip(_ kind: Int32, _ a: Int32, _ b: Int32, _ c: Int32, _ d: Int32)
            throws -> WatcherRelayCommand
        {
            var buffer = [UInt8](repeating: 0, count: 256)
            let written = opc_relay_command_encode(kind, a, b, c, d, &buffer, buffer.count)
            #expect(written > 0)
            return try JSONDecoder().decode(
                WatcherRelayCommand.self, from: Data(buffer.prefix(Int(written))))
        }
        #expect(try roundTrip(OPC_RELAY_COMMAND_TOGGLE_RECORDING, 0, 0, 0, 0) == .toggleRecording)
        #expect(try roundTrip(OPC_RELAY_COMMAND_SET_ISO, 400, 0, 0, 0) == .setISO(400))
        #expect(try roundTrip(OPC_RELAY_COMMAND_SET_ZOOM, 250, 0, 0, 0) == .setZoom(250))
        #expect(
            try roundTrip(OPC_RELAY_COMMAND_TAP_FOCUS, 500, 250, 1000, 1000)
                == .tapFocus(
                    cameraX: 500, cameraY: 250, coordinateWidth: 1000, coordinateHeight: 1000))
        #expect(
            try roundTrip(OPC_RELAY_COMMAND_SET_WHITE_BALANCE, 1, 4200, 20, 0)
                == .setWhiteBalance(mode: 1, kelvin: 4200, tint: 20))
        var buffer = [UInt8](repeating: 0, count: 32)
        #expect(
            opc_relay_command_encode(99, 0, 0, 0, 0, &buffer, buffer.count)
                == Int64(OPC_RELAY_ERR_OUT_OF_RANGE))
    }

    @Test
    func focusMapMirrorsAcrossTheFittedPicture() {
        var point = OpcRelayFocusPoint()
        #expect(opc_relay_focus_map(320, 180, 1280, 720, 0, &point) == OPC_RELAY_OK)
        #expect(point.x == 250)
        #expect(point.y == 250)
        #expect(opc_relay_focus_map(320, 180, 1280, 720, 1, &point) == OPC_RELAY_OK)
        #expect(point.x == 750)
        #expect(
            opc_relay_focus_map(-1, 180, 1280, 720, 0, &point) == OPC_RELAY_ERR_OUT_OF_RANGE)
    }

    @Test
    func protocolInfoMirrorsTheCoreConstants() {
        var info = OpcRelayProtocolInfo()
        #expect(opc_relay_protocol_info(&info) == OPC_RELAY_OK)
        #expect(info.version == Int32(WatcherRelayProtocol.version))
        #expect(info.framing_header_bytes == Int32(WatcherRelayFraming.headerBytes))
        #expect(info.max_payload_bytes == Int32(WatcherRelayProtocol.maximumPayloadBytes))
        #expect(info.max_retries == Int32(WatcherRelayRecovery.maximumRetries))
        #expect(info.join_timeout_ms == 15_000)
        #expect(info.silence_timeout_ms == 5_000)
        #expect(text(&info.service_type) == WatcherRelayProtocol.serviceType)
        #expect(text(&info.txt_camera) == WatcherRelayProtocol.txtCamera)
    }

    private func text<T>(_ field: inout T) -> String {
        withUnsafePointer(to: &field) {
            String(cString: UnsafeRawPointer($0).assumingMemoryBound(to: CChar.self))
        }
    }
}
