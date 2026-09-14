#include "opc_decode.h"

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/frame.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>

struct OpcDecoder {
    AVCodecContext *context;
    AVPacket *packet;
    AVFrame *frame;
};

OpcDecoder *opc_decoder_create(int32_t codec) {
    enum AVCodecID id;
    switch (codec) {
        case OPC_DECODE_CODEC_HEVC: id = AV_CODEC_ID_HEVC; break;
        case OPC_DECODE_CODEC_H264: id = AV_CODEC_ID_H264; break;
        default: return NULL;
    }

    const AVCodec *implementation = avcodec_find_decoder(id);
    if (!implementation) {
        return NULL;
    }

    OpcDecoder *decoder = calloc(1, sizeof(OpcDecoder));
    if (!decoder) {
        return NULL;
    }

    decoder->context = avcodec_alloc_context3(implementation);
    decoder->packet = av_packet_alloc();
    decoder->frame = av_frame_alloc();
    if (!decoder->context || !decoder->packet || !decoder->frame) {
        opc_decoder_destroy(decoder);
        return NULL;
    }

    // The camera's stream has no B-frames and the relay re-encode keeps it that way, so
    // reordering only costs latency. Slice threading keeps the parallelism without the
    // frame-of-delay that frame threading adds.
    decoder->context->flags |= AV_CODEC_FLAG_LOW_DELAY;
    decoder->context->thread_type = FF_THREAD_SLICE;
    decoder->context->has_b_frames = 0;

    if (avcodec_open2(decoder->context, implementation, NULL) < 0) {
        opc_decoder_destroy(decoder);
        return NULL;
    }
    return decoder;
}

void opc_decoder_destroy(OpcDecoder *decoder) {
    if (!decoder) {
        return;
    }
    if (decoder->frame) {
        av_frame_free(&decoder->frame);
    }
    if (decoder->packet) {
        av_packet_free(&decoder->packet);
    }
    if (decoder->context) {
        avcodec_free_context(&decoder->context);
    }
    free(decoder);
}

int32_t opc_decoder_send(OpcDecoder *decoder, const uint8_t *data, size_t length) {
    if (!decoder || !decoder->context) {
        return OPC_DECODE_ERR_NULL;
    }
    if (!data || length == 0) {
        return OPC_DECODE_ERR_NULL;
    }
    if (length > (size_t)INT_MAX) {
        return OPC_DECODE_ERR_SEND;
    }

    // FFmpeg may retain a packet until a later receive call. The Rust access-unit
    // slice is borrowed and is dropped immediately after `Decoder::send`, so packet
    // data must be reference-counted here rather than pointing at that short-lived
    // allocation. Otherwise delayed HEVC parsing sees recycled bytes as NAL headers.
    av_packet_unref(decoder->packet);
    if (av_new_packet(decoder->packet, (int)length) < 0) {
        return OPC_DECODE_ERR_SEND;
    }
    memcpy(decoder->packet->data, data, length);

    int status = avcodec_send_packet(decoder->context, decoder->packet);
    if (status < 0 && status != AVERROR(EAGAIN)) {
        return OPC_DECODE_ERR_SEND;
    }
    return OPC_DECODE_OK;
}

int32_t opc_decoder_receive(OpcDecoder *decoder, OpcDecodedFrame *out) {
    if (!decoder || !decoder->context || !out) {
        return OPC_DECODE_ERR_NULL;
    }

    av_frame_unref(decoder->frame);
    int status = avcodec_receive_frame(decoder->context, decoder->frame);
    if (status == AVERROR(EAGAIN) || status == AVERROR_EOF) {
        return OPC_DECODE_AGAIN;
    }
    if (status < 0) {
        return OPC_DECODE_ERR_RECEIVE;
    }

    memset(out, 0, sizeof(*out));
    out->width = decoder->frame->width;
    out->height = decoder->frame->height;
    out->format = decoder->frame->format == AV_PIX_FMT_YUV420P ? OPC_DECODE_FORMAT_YUV420P
                                                               : OPC_DECODE_FORMAT_OTHER;
#ifdef AV_FRAME_FLAG_KEY
    out->is_keyframe = (decoder->frame->flags & AV_FRAME_FLAG_KEY) ? 1 : 0;
#else
    out->is_keyframe = decoder->frame->key_frame ? 1 : 0;
#endif
    for (int plane = 0; plane < 3; plane++) {
        out->plane[plane] = decoder->frame->data[plane];
        out->stride[plane] = decoder->frame->linesize[plane];
    }
    return OPC_DECODE_FRAME;
}

void opc_decoder_flush(OpcDecoder *decoder) {
    if (decoder && decoder->context) {
        avcodec_flush_buffers(decoder->context);
    }
}

// ── Files ─────────────────────────────────────────────────────────────────────

struct OpcFileReader {
    AVFormatContext *format;
    AVCodecContext *context;
    AVPacket *packet;
    AVFrame *frame;
    int stream;
    int at_end;
};

OpcFileReader *opc_file_open(const char *path) {
    if (!path) {
        return NULL;
    }
    OpcFileReader *reader = calloc(1, sizeof(OpcFileReader));
    if (!reader) {
        return NULL;
    }
    reader->stream = -1;
    if (avformat_open_input(&reader->format, path, NULL, NULL) < 0) {
        opc_file_close(reader);
        return NULL;
    }
    if (avformat_find_stream_info(reader->format, NULL) < 0) {
        opc_file_close(reader);
        return NULL;
    }
    const AVCodec *implementation = NULL;
    int stream = av_find_best_stream(reader->format, AVMEDIA_TYPE_VIDEO, -1, -1, &implementation, 0);
    if (stream < 0 || !implementation) {
        opc_file_close(reader);
        return NULL;
    }
    reader->stream = stream;
    reader->context = avcodec_alloc_context3(implementation);
    reader->packet = av_packet_alloc();
    reader->frame = av_frame_alloc();
    if (!reader->context || !reader->packet || !reader->frame) {
        opc_file_close(reader);
        return NULL;
    }
    if (avcodec_parameters_to_context(reader->context, reader->format->streams[stream]->codecpar) < 0) {
        opc_file_close(reader);
        return NULL;
    }
    // Frame threading: a file is decoded ahead of the clock, so the extra latency is free.
    reader->context->thread_count = 0;
    if (avcodec_open2(reader->context, implementation, NULL) < 0) {
        opc_file_close(reader);
        return NULL;
    }
    return reader;
}

void opc_file_close(OpcFileReader *reader) {
    if (!reader) {
        return;
    }
    if (reader->frame) {
        av_frame_free(&reader->frame);
    }
    if (reader->packet) {
        av_packet_free(&reader->packet);
    }
    if (reader->context) {
        avcodec_free_context(&reader->context);
    }
    if (reader->format) {
        avformat_close_input(&reader->format);
    }
    free(reader);
}

int32_t opc_file_info(OpcFileReader *reader, OpcFileInfo *out) {
    if (!reader || !reader->format || !out) {
        return OPC_DECODE_ERR_NULL;
    }
    AVStream *stream = reader->format->streams[reader->stream];
    memset(out, 0, sizeof(*out));
    out->width = reader->context->width;
    out->height = reader->context->height;
    AVRational rate = av_guess_frame_rate(reader->format, stream, NULL);
    out->fps_num = rate.num;
    out->fps_den = rate.den;
    if (stream->duration > 0) {
        out->duration_ms = av_rescale_q(stream->duration, stream->time_base, (AVRational){1, 1000});
    } else if (reader->format->duration > 0) {
        out->duration_ms = reader->format->duration / (AV_TIME_BASE / 1000);
    }
    return OPC_DECODE_OK;
}

static int32_t fill_frame(OpcFileReader *reader, OpcDecodedFrame *out, int64_t *pts_ms) {
    AVStream *stream = reader->format->streams[reader->stream];
    memset(out, 0, sizeof(*out));
    out->width = reader->frame->width;
    out->height = reader->frame->height;
    out->format = reader->frame->format == AV_PIX_FMT_YUV420P ? OPC_DECODE_FORMAT_YUV420P
                                                              : OPC_DECODE_FORMAT_OTHER;
#ifdef AV_FRAME_FLAG_KEY
    out->is_keyframe = (reader->frame->flags & AV_FRAME_FLAG_KEY) ? 1 : 0;
#else
    out->is_keyframe = reader->frame->key_frame ? 1 : 0;
#endif
    for (int plane = 0; plane < 3; plane++) {
        out->plane[plane] = reader->frame->data[plane];
        out->stride[plane] = reader->frame->linesize[plane];
    }
    int64_t pts = reader->frame->pts != AV_NOPTS_VALUE ? reader->frame->pts : reader->frame->pkt_dts;
    *pts_ms = pts == AV_NOPTS_VALUE ? 0 : av_rescale_q(pts, stream->time_base, (AVRational){1, 1000});
    return OPC_DECODE_FRAME;
}

int32_t opc_file_next(OpcFileReader *reader, OpcDecodedFrame *out, int64_t *pts_ms) {
    if (!reader || !reader->context || !out || !pts_ms) {
        return OPC_DECODE_ERR_NULL;
    }
    for (;;) {
        av_frame_unref(reader->frame);
        int status = avcodec_receive_frame(reader->context, reader->frame);
        if (status == 0) {
            return fill_frame(reader, out, pts_ms);
        }
        if (status == AVERROR_EOF) {
            return OPC_FILE_END;
        }
        if (status != AVERROR(EAGAIN)) {
            return OPC_DECODE_ERR_RECEIVE;
        }
        if (reader->at_end) {
            return OPC_FILE_END;
        }
        // Feed the next packet of our stream; at the end of the file drain the decoder.
        av_packet_unref(reader->packet);
        int read = av_read_frame(reader->format, reader->packet);
        if (read < 0) {
            reader->at_end = 1;
            avcodec_send_packet(reader->context, NULL);
            continue;
        }
        if (reader->packet->stream_index != reader->stream) {
            av_packet_unref(reader->packet);
            continue;
        }
        int sent = avcodec_send_packet(reader->context, reader->packet);
        av_packet_unref(reader->packet);
        if (sent < 0 && sent != AVERROR(EAGAIN)) {
            return OPC_DECODE_ERR_SEND;
        }
    }
}

int32_t opc_file_seek(OpcFileReader *reader, int64_t position_ms) {
    if (!reader || !reader->format) {
        return OPC_DECODE_ERR_NULL;
    }
    AVStream *stream = reader->format->streams[reader->stream];
    int64_t target = av_rescale_q(position_ms, (AVRational){1, 1000}, stream->time_base);
    if (av_seek_frame(reader->format, reader->stream, target, AVSEEK_FLAG_BACKWARD) < 0) {
        // A raw elementary stream has no index to seek by time; going back to the
        // start by byte is always possible and is what a replay wants.
        if (position_ms != 0 ||
            av_seek_frame(reader->format, reader->stream, 0, AVSEEK_FLAG_BYTE | AVSEEK_FLAG_BACKWARD) < 0) {
            return OPC_DECODE_ERR_SEND;
        }
    }
    avcodec_flush_buffers(reader->context);
    reader->at_end = 0;
    return OPC_DECODE_OK;
}
