// Minimal C surface over libavcodec for the desktop watcher.
//
// Deliberately hand-written rather than generated: the ABI is six functions wide, and
// keeping it explicit means the Windows build only has to point at a prebuilt FFmpeg
// instead of carrying bindgen and libclang.

#ifndef OPC_DECODE_H
#define OPC_DECODE_H

#include <stddef.h>
#include <stdint.h>

#define OPC_DECODE_CODEC_HEVC 0
#define OPC_DECODE_CODEC_H264 1

#define OPC_DECODE_OK 0
#define OPC_DECODE_FRAME 1
#define OPC_DECODE_AGAIN 2
#define OPC_DECODE_ERR_UNSUPPORTED (-1)
#define OPC_DECODE_ERR_NULL (-2)
#define OPC_DECODE_ERR_SEND (-3)
#define OPC_DECODE_ERR_RECEIVE (-4)

// 8-bit planar 4:2:0, which is what the relay's VideoToolbox encode produces.
#define OPC_DECODE_FORMAT_YUV420P 0
#define OPC_DECODE_FORMAT_OTHER (-1)

typedef struct OpcDecoder OpcDecoder;

/// Borrowed view of the decoder's current picture. Valid only until the next
/// `opc_decoder_send`, `opc_decoder_receive`, `opc_decoder_flush`, or destroy.
typedef struct {
    int32_t width;
    int32_t height;
    int32_t format;
    int32_t is_keyframe;
    const uint8_t *plane[3];
    int32_t stride[3];
} OpcDecodedFrame;

OpcDecoder *opc_decoder_create(int32_t codec);
void opc_decoder_destroy(OpcDecoder *decoder);

/// Feeds one Annex-B access unit.
int32_t opc_decoder_send(OpcDecoder *decoder, const uint8_t *data, size_t length);

/// Pulls the next picture. `OPC_DECODE_FRAME` filled `out`; `OPC_DECODE_AGAIN` means
/// feed more.
int32_t opc_decoder_receive(OpcDecoder *decoder, OpcDecodedFrame *out);

/// Drops decoder state after a reconnect, the way the shells flush for recovery.
void opc_decoder_flush(OpcDecoder *decoder);

// ── Files ─────────────────────────────────────────────────────────────────────
// A clip on disk — the camera's 720p LRF proxy, or an original — demuxed and decoded
// picture by picture, for the media player. Only the first video stream is read.

typedef struct OpcFileReader OpcFileReader;

typedef struct {
    int32_t width;
    int32_t height;
    /// Frame rate as a rational, e.g. 30000/1001.
    int32_t fps_num;
    int32_t fps_den;
    /// Whole clip, in milliseconds; zero when the container does not say.
    int64_t duration_ms;
} OpcFileInfo;

#define OPC_FILE_END 3

/// Opens a file for reading. NULL when it has no decodable video stream.
OpcFileReader *opc_file_open(const char *path);
void opc_file_close(OpcFileReader *reader);
int32_t opc_file_info(OpcFileReader *reader, OpcFileInfo *out);

/// Decodes the next picture. `OPC_DECODE_FRAME` filled `out` and `pts_ms`;
/// `OPC_FILE_END` means the clip is over.
int32_t opc_file_next(OpcFileReader *reader, OpcDecodedFrame *out, int64_t *pts_ms);

/// Seeks to the keyframe at or before `position_ms` and flushes the decoder.
int32_t opc_file_seek(OpcFileReader *reader, int64_t position_ms);

#endif
