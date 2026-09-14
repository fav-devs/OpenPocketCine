import COpcDesktop
import Foundation
import OpenPocketViewCore

// The media catalogue for the desktop shell.
//
// `MediaManifest` knows every byte layout the bodies use, including the Pocket 3's
// markerless stills and the base + step handle fit that keeps a delete from landing on
// the neighbouring file. The shell collects the `0x00/0x27` chunks and hands the blobs
// over; it reads the result as JSON and parses nothing itself.

/// Decodes one page of the catalogue. `sd` and `internal` are the reassembled chunk
/// payloads for counters 1 and 2 (either may be empty); `merged` is every chunk in
/// counter order. Writes a JSON array of `MediaFile` and reports the size needed, the
/// same way every other emitter here does.
@_cdecl("opc_media_decode")
func opc_media_decode(
    _ sd: UnsafePointer<UInt8>?, _ sdCount: Int, _ `internal`: UnsafePointer<UInt8>?,
    _ internalCount: Int, _ merged: UnsafePointer<UInt8>?, _ mergedCount: Int,
    _ out: UnsafeMutablePointer<UInt8>?, _ capacity: Int
) -> Int64 {
    func bytes(_ pointer: UnsafePointer<UInt8>?, _ count: Int) -> [UInt8] {
        guard let pointer, count > 0 else { return [] }
        return Array(DesktopFacade.borrow(pointer, count))
    }
    let files = MediaManifest.decodeStores(
        sd: bytes(sd, sdCount), `internal`: bytes(`internal`, internalCount),
        merged: bytes(merged, mergedCount))
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    guard let json = try? encoder.encode(files) else {
        return Int64(OPC_RELAY_ERR_OUT_OF_RANGE)
    }
    return DesktopFacade.emit(json, into: out, capacity: capacity)
}
