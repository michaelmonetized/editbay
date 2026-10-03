#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/channel_layout.h>
#include <libavutil/imgutils.h>
#include <libswscale/swscale.h>
#include <libswresample/swresample.h>

typedef struct {
    int width, height, rate_num, rate_den, sample_rate, channels;
    int64_t duration_ns;
    int color_primaries, color_transfer, color_matrix, color_range;
} EBInfo;

typedef struct {
    AVFormatContext *format;
    AVCodecContext *codec;
    AVPacket *packet;
    AVFrame *frame;
    struct SwsContext *scale;
    SwrContext *resample;
    int stream, audio, draining;
    AVRational time_base;
} EBReader;

void eb_reader_close(EBReader *r) {
    if (!r) return;
    sws_freeContext(r->scale);
    swr_free(&r->resample);
    av_frame_free(&r->frame);
    av_packet_free(&r->packet);
    avcodec_free_context(&r->codec);
    avformat_close_input(&r->format);
    free(r);
}

EBReader *eb_reader_open(const char *path, int audio, EBInfo *info, int *error) {
    EBReader *r = calloc(1, sizeof(*r));
    AVDictionary *options = NULL;
    const AVCodec *decoder = NULL;
    int ret = AVERROR(ENOMEM);
    if (!r) goto failed;
    r->audio = audio;
    av_dict_set(&options, "protocol_whitelist", "file", 0);
    ret = avformat_open_input(&r->format, path, NULL, &options);
    av_dict_free(&options);
    if (ret < 0) goto failed;
    if ((ret = avformat_find_stream_info(r->format, NULL)) < 0) goto failed;
    ret = av_find_best_stream(r->format, audio ? AVMEDIA_TYPE_AUDIO : AVMEDIA_TYPE_VIDEO, -1, -1, &decoder, 0);
    if (ret < 0) goto failed;
    r->stream = ret;
    AVStream *stream = r->format->streams[r->stream];
    r->time_base = stream->time_base;
    r->codec = avcodec_alloc_context3(decoder);
    if (!r->codec) { ret = AVERROR(ENOMEM); goto failed; }
    if ((ret = avcodec_parameters_to_context(r->codec, stream->codecpar)) < 0) goto failed;
    r->codec->thread_count = 2;
    if ((ret = avcodec_open2(r->codec, decoder, NULL)) < 0) goto failed;
    r->packet = av_packet_alloc();
    r->frame = av_frame_alloc();
    if (!r->packet || !r->frame) { ret = AVERROR(ENOMEM); goto failed; }
    memset(info, 0, sizeof(*info));
    info->duration_ns = r->format->duration == AV_NOPTS_VALUE ? -1 : av_rescale_q(r->format->duration, AV_TIME_BASE_Q, (AVRational){1,1000000000});
    info->color_primaries = r->codec->color_primaries;
    info->color_transfer = r->codec->color_trc;
    info->color_matrix = r->codec->colorspace;
    info->color_range = r->codec->color_range;
    if (audio) {
        AVChannelLayout stereo = AV_CHANNEL_LAYOUT_STEREO;
        info->sample_rate = 48000;
        info->channels = 2;
        ret = swr_alloc_set_opts2(&r->resample, &stereo, AV_SAMPLE_FMT_FLT, 48000,
            &r->codec->ch_layout, r->codec->sample_fmt, r->codec->sample_rate, 0, NULL);
        if (ret < 0 || (ret = swr_init(r->resample)) < 0) goto failed;
    } else {
        info->width = r->codec->width;
        info->height = r->codec->height;
        AVRational rate = av_guess_frame_rate(r->format, stream, NULL);
        info->rate_num = rate.num;
        info->rate_den = rate.den;
        if (info->width <= 0 || info->height <= 0 || info->width > 8192 || info->height > 8192) { ret = AVERROR(EINVAL); goto failed; }
    }
    *error = 0;
    return r;
failed:
    av_dict_free(&options);
    eb_reader_close(r);
    *error = ret;
    return NULL;
}

static int receive(EBReader *r) {
    for (;;) {
        int ret = avcodec_receive_frame(r->codec, r->frame);
        if (ret != AVERROR(EAGAIN)) return ret;
        if (r->draining) return AVERROR_EOF;
        do {
            ret = av_read_frame(r->format, r->packet);
            if (ret < 0) break;
            if (r->packet->stream_index == r->stream) break;
            av_packet_unref(r->packet);
        } while (1);
        if (ret == AVERROR_EOF) {
            r->draining = 1;
            ret = avcodec_send_packet(r->codec, NULL);
        } else if (ret >= 0) {
            ret = avcodec_send_packet(r->codec, r->packet);
            av_packet_unref(r->packet);
        }
        if (ret < 0 && ret != AVERROR_EOF) return ret;
    }
}

int eb_reader_next(EBReader *r, uint8_t *output, size_t capacity, int64_t *pts_ns) {
next_frame:
    int ret = receive(r);
    if (ret == AVERROR_EOF) {
        if (r->audio) {
            uint8_t *planes[] = {output};
            int count = swr_convert(r->resample, planes, (int)(capacity / 8), NULL, 0);
            *pts_ns = INT64_MIN;
            return count < 0 ? count : count * 8;
        }
        return 0;
    }
    if (ret < 0) return ret;
    *pts_ns = r->frame->best_effort_timestamp == AV_NOPTS_VALUE ? INT64_MIN :
        av_rescale_q(r->frame->best_effort_timestamp, r->time_base, (AVRational){1,1000000000});
    if (r->audio) {
        int count = swr_get_out_samples(r->resample, r->frame->nb_samples);
        if (count < 0 || (size_t)count * 8 > capacity) return AVERROR(ENOBUFS);
        uint8_t *planes[] = {output};
        count = swr_convert(r->resample, planes, count, (const uint8_t **)r->frame->extended_data, r->frame->nb_samples);
        if (count == 0) goto next_frame;
        return count < 0 ? count : count * 8;
    }
    int width = r->codec->width, height = r->codec->height;
    if (r->frame->width != width || r->frame->height != height || capacity < (size_t)width * height * 4) return AVERROR(ENOBUFS);
    r->scale = sws_getCachedContext(r->scale, width, height, r->frame->format, width, height,
        AV_PIX_FMT_RGBA, SWS_BILINEAR, NULL, NULL, NULL);
    if (!r->scale) return AVERROR(ENOMEM);
    int matrix = r->frame->colorspace == AVCOL_SPC_BT709 || (r->frame->colorspace == AVCOL_SPC_UNSPECIFIED && (width >= 1280 || height >= 720)) ? SWS_CS_ITU709 : SWS_CS_ITU601;
    const int *coeff = sws_getCoefficients(matrix);
    ret = sws_setColorspaceDetails(r->scale, coeff, r->frame->color_range == AVCOL_RANGE_JPEG, coeff, 1, 0, 1<<16, 1<<16);
    if (ret < 0) return ret;
    uint8_t *planes[] = {output};
    int strides[] = {width * 4};
    ret = sws_scale(r->scale, (const uint8_t * const *)r->frame->data, r->frame->linesize, 0, height, planes, strides);
    return ret < 0 ? ret : width * height * 4;
}

typedef struct {
    AVFormatContext *format;
    AVCodecContext *codec;
    AVStream *stream;
    AVPacket *packet;
    AVFrame *frame;
    int64_t frames;
} EBWriter;

void eb_writer_close(EBWriter *w) {
    if (!w) return;
    av_frame_free(&w->frame);
    av_packet_free(&w->packet);
    avcodec_free_context(&w->codec);
    if (w->format) {
        if (w->format->pb) avio_closep(&w->format->pb);
        avformat_free_context(w->format);
    }
    free(w);
}

EBWriter *eb_writer_open(const char *path, int width, int height, int num, int den, int *error) {
    EBWriter *w = calloc(1, sizeof(*w));
    int ret = AVERROR(ENOMEM);
    if (!w) goto failed;
    if ((ret = avformat_alloc_output_context2(&w->format, NULL, "matroska", path)) < 0) goto failed;
    const AVCodec *encoder = avcodec_find_encoder(AV_CODEC_ID_FFV1);
    if (!encoder) { ret = AVERROR_ENCODER_NOT_FOUND; goto failed; }
    w->codec = avcodec_alloc_context3(encoder);
    w->stream = avformat_new_stream(w->format, NULL);
    w->frame = av_frame_alloc();
    w->packet = av_packet_alloc();
    if (!w->codec || !w->stream || !w->frame || !w->packet) { ret = AVERROR(ENOMEM); goto failed; }
    w->codec->width = width;
    w->codec->height = height;
    w->codec->pix_fmt = AV_PIX_FMT_BGRA;
    w->codec->time_base = (AVRational){den, num};
    w->codec->framerate = (AVRational){num, den};
    w->codec->thread_count = 2;
    w->codec->color_range = AVCOL_RANGE_JPEG;
    w->codec->color_primaries = AVCOL_PRI_BT709;
    w->codec->color_trc = AVCOL_TRC_IEC61966_2_1;
    w->codec->colorspace = AVCOL_SPC_RGB;
    if (w->format->oformat->flags & AVFMT_GLOBALHEADER) w->codec->flags |= AV_CODEC_FLAG_GLOBAL_HEADER;
    if ((ret = avcodec_open2(w->codec, encoder, NULL)) < 0) goto failed;
    if ((ret = avcodec_parameters_from_context(w->stream->codecpar, w->codec)) < 0) goto failed;
    w->stream->time_base = w->codec->time_base;
    if ((ret = avio_open(&w->format->pb, path, AVIO_FLAG_WRITE)) < 0) goto failed;
    if ((ret = avformat_write_header(w->format, NULL)) < 0) goto failed;
    w->frame->format = w->codec->pix_fmt;
    w->frame->width = width;
    w->frame->height = height;
    if ((ret = av_frame_get_buffer(w->frame, 32)) < 0) goto failed;
    *error = 0;
    return w;
failed:
    eb_writer_close(w);
    *error = ret;
    return NULL;
}

static int packets(EBWriter *w) {
    for (;;) {
        int ret = avcodec_receive_packet(w->codec, w->packet);
        if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF) return 0;
        if (ret < 0) return ret;
        av_packet_rescale_ts(w->packet, w->codec->time_base, w->stream->time_base);
        w->packet->stream_index = w->stream->index;
        ret = av_interleaved_write_frame(w->format, w->packet);
        av_packet_unref(w->packet);
        if (ret < 0) return ret;
    }
}

int eb_writer_frame(EBWriter *w, const uint8_t *rgba, size_t length) {
    if (length != (size_t)w->codec->width * w->codec->height * 4) return AVERROR(EINVAL);
    int ret = av_frame_make_writable(w->frame);
    if (ret < 0) return ret;
    for (int y = 0; y < w->codec->height; y++) {
        uint8_t *row = w->frame->data[0] + y * w->frame->linesize[0];
        const uint8_t *source = rgba + (size_t)y * w->codec->width * 4;
        for (int x = 0; x < w->codec->width; x++) {
            row[x*4] = source[x*4+2]; row[x*4+1] = source[x*4+1];
            row[x*4+2] = source[x*4]; row[x*4+3] = source[x*4+3];
        }
    }
    w->frame->pts = w->frames++;
    if ((ret = avcodec_send_frame(w->codec, w->frame)) < 0) return ret;
    return packets(w);
}

int eb_writer_finish(EBWriter *w) {
    int ret = avcodec_send_frame(w->codec, NULL);
    if (ret < 0) return ret;
    if ((ret = packets(w)) < 0) return ret;
    return av_write_trailer(w->format);
}

void eb_error(int code, char *message, size_t length) { av_strerror(code, message, length); }
const char *eb_version(void) { return av_version_info(); }
