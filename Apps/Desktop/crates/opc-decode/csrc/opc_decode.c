#include "opc_decode.h"

#include <libavcodec/avcodec.h>
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
