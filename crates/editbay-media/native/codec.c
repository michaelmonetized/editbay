#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/stat.h>
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/channel_layout.h>
#include <libavutil/imgutils.h>
#include <libavutil/mathematics.h>
#include <libavutil/audio_fifo.h>
#include <libavutil/opt.h>
#include <libswscale/swscale.h>
#include <libswresample/swresample.h>
#include "color_metadata.h"


typedef struct EBCancel EBCancel;

typedef struct {
    AVFormatContext *format;
    AVCodecContext *codec;
    AVStream *stream;
    AVPacket *packet;
    AVFrame *frame;
    int64_t frames;
    AVCodecContext *audio;
    AVStream *sound;
    AVFrame *samples;
    int64_t sound_frames;
    struct SwsContext *conversion;
    AVAudioFifo *fifo;
    int encoding;
    int64_t sound_end;
    int sound_final;
    EBCancel *cancel;
    FILE *file;
} EBWriter;

int eb_cancelled(EBCancel *cancel);

static const AVCodec *delivery_encoder(int encoding) {
    if (encoding == 1) return avcodec_find_encoder_by_name("libx264");
    if (encoding == 2) return avcodec_find_encoder_by_name("prores_ks");
    if (encoding == 4 || encoding == 5) return avcodec_find_encoder(AV_CODEC_ID_DNXHD);
    if (encoding == 6) return avcodec_find_encoder_by_name("libx265");
    return avcodec_find_encoder(AV_CODEC_ID_PNG);
}

static enum AVPixelFormat delivery_pixels(int encoding) {
    if (encoding == 1) return AV_PIX_FMT_YUV420P;
    if (encoding == 2) return AV_PIX_FMT_YUVA444P10LE;
    if (encoding == 3) return AV_PIX_FMT_RGBA64BE;
    if (encoding == 4 || encoding == 5) return AV_PIX_FMT_YUV422P10LE;
    if (encoding == 6) return AV_PIX_FMT_YUV420P10LE;
    return AV_PIX_FMT_RGBA;
}

static const char *delivery_container(int encoding) {
    return encoding == 1 || encoding == 6 ? "mp4" : encoding == 5 ? "mxf" : "mov";
}

static enum AVCodecID delivery_audio(int encoding) {
    return encoding == 1 || encoding == 6 ? AV_CODEC_ID_AAC :
        encoding == 4 || encoding == 5 ? AV_CODEC_ID_PCM_S24LE : AV_CODEC_ID_PCM_F32LE;
}

int eb_delivery_available(int encoding) {
    if (encoding < 0 || encoding > 6) return 7;
    const AVCodec *encoder = delivery_encoder(encoding);
    const AVCodec *sound = avcodec_find_encoder(delivery_audio(encoding));
    const AVOutputFormat *muxer = av_guess_format(delivery_container(encoding), NULL, NULL);
    if (!encoder) return 1;
    if (!sound) return 2;
    if (!muxer) return 3;
    if (!avcodec_find_decoder(encoder->id) || !avcodec_find_decoder(sound->id)) return 4;
    const enum AVPixelFormat *formats = NULL;
    int count = 0;
    if (avcodec_get_supported_config(NULL, encoder, AV_CODEC_CONFIG_PIX_FORMAT, 0,
            (const void **)&formats, &count) < 0 || !formats || count < 1 || count > 256) return 7;
    int found = 0;
    for (int i = 0; i < count; i++) found |= formats[i] == delivery_pixels(encoding);
    if (!found) return 5;
    int picture_mux = avformat_query_codec(muxer, encoder->id, FF_COMPLIANCE_NORMAL);
    int sound_mux = avformat_query_codec(muxer, sound->id, FF_COMPLIANCE_NORMAL);
    if (!picture_mux || !sound_mux) return 6;
    return picture_mux < 0 || sound_mux < 0 ? 7 : 0;
}

void eb_writer_close(EBWriter *w) {
    if (!w) return;
    av_frame_free(&w->frame);
    av_frame_free(&w->samples);
    av_packet_free(&w->packet);
    avcodec_free_context(&w->codec);
    avcodec_free_context(&w->audio);
    sws_freeContext(w->conversion);
    av_audio_fifo_free(w->fifo);
    if (w->format) {
        if (w->format->pb) {
            av_freep(&w->format->pb->buffer);
            avio_context_free(&w->format->pb);
        }
        avformat_free_context(w->format);
    }
    if (w->file) fclose(w->file);
    free(w);
}

static int write_bytes(void *opaque, const uint8_t *bytes, int length) {
    EBWriter *w = opaque;
    if (w->cancel && eb_cancelled(w->cancel)) return AVERROR_EXIT;
    return fwrite(bytes, 1, length, w->file) == (size_t)length ? length : AVERROR(EIO);
}

static int64_t seek_bytes(void *opaque, int64_t offset, int whence) {
    EBWriter *w = opaque;
    if (w->cancel && eb_cancelled(w->cancel)) return AVERROR_EXIT;
    FILE *file = w->file;
    if (whence == AVSEEK_SIZE) {
        struct stat info;
        return fstat(fileno(file), &info) == 0 ? info.st_size : AVERROR(errno);
    }
    if (fseeko(file, offset, whence & ~AVSEEK_FORCE) != 0) return AVERROR(errno);
    return ftello(file);
}

static EBWriter *writer_open(int descriptor, int width, int height, int num, int den,
        int rate, const char *layout, int encoding, const EBColor *color, void *cancel, int *error) {
    EBWriter *w = calloc(1, sizeof(*w));
    AVDictionary *options = NULL;
    int ret = AVERROR(ENOMEM);
    if (!w) goto failed;
    w->cancel = cancel;
    w->encoding = encoding;
    if (encoding < 0 || encoding > 6) { ret = AVERROR(EINVAL); goto failed; }
    int dnx = encoding == 4 || encoding == 5;
    int hevc = encoding == 6;
    int aac = encoding == 1 || hevc;
    struct stat file_info;
    if (fstat(descriptor, &file_info) != 0 || !S_ISREG(file_info.st_mode) || file_info.st_size != 0) {
        ret = AVERROR(EINVAL); goto failed;
    }
    int owned = fcntl(descriptor, F_DUPFD_CLOEXEC, 0);
    if (owned < 0) { ret = AVERROR(errno); goto failed; }
    w->file = fdopen(owned, "wb");
    if (!w->file) { close(owned); ret = AVERROR(errno); goto failed; }
    if (fseeko(w->file, 0, SEEK_SET)) { ret = AVERROR(errno); goto failed; }
    if ((ret = avformat_alloc_output_context2(&w->format, NULL,
            layout ? delivery_container(encoding) : "matroska", NULL)) < 0) goto failed;
    unsigned char *buffer = av_malloc(32768);
    if (!buffer) { ret = AVERROR(ENOMEM); goto failed; }
    w->format->pb = avio_alloc_context(buffer, 32768, 1, w, NULL, write_bytes, seek_bytes);
    if (!w->format->pb) { av_free(buffer); ret = AVERROR(ENOMEM); goto failed; }
    w->format->flags |= AVFMT_FLAG_CUSTOM_IO;
    const AVCodec *encoder = layout ? delivery_encoder(encoding) : avcodec_find_encoder(AV_CODEC_ID_FFV1);
    if (!encoder) { ret = AVERROR_ENCODER_NOT_FOUND; goto failed; }
    w->codec = avcodec_alloc_context3(encoder);
    w->stream = avformat_new_stream(w->format, NULL);
    w->frame = av_frame_alloc();
    w->packet = av_packet_alloc();
    if (!w->codec || !w->stream || !w->frame || !w->packet) { ret = AVERROR(ENOMEM); goto failed; }
    w->codec->width = width;
    w->codec->height = height;
    w->codec->pix_fmt = layout ? delivery_pixels(encoding) : AV_PIX_FMT_BGRA;
    w->codec->time_base = (AVRational){den, num};
    w->codec->framerate = (AVRational){num, den};
    w->codec->thread_count = 2;
    w->codec->color_range = encoding == 1 || encoding == 2 || dnx || hevc ? AVCOL_RANGE_MPEG : AVCOL_RANGE_JPEG;
    w->codec->color_primaries = color ? color->primaries : AVCOL_PRI_BT709;
    w->codec->color_trc = color ? color->transfer : AVCOL_TRC_IEC61966_2_1;
    w->codec->colorspace = hevc && color && color->primaries == 9 ? AVCOL_SPC_BT2020_NCL :
        encoding == 1 || encoding == 2 || dnx || hevc ? AVCOL_SPC_BT709 : AVCOL_SPC_RGB;
    w->codec->alpha_mode = aac || dnx ? AVALPHA_MODE_UNSPECIFIED : AVALPHA_MODE_STRAIGHT;
    if (encoding == 1) {
        w->codec->gop_size = 48;
        w->codec->max_b_frames = 0;
        if ((ret = av_opt_set(w->codec->priv_data, "preset", "medium", 0)) < 0) goto failed;
        if ((ret = av_opt_set(w->codec->priv_data, "crf", "16", 0)) < 0) goto failed;
    } else if (encoding == 2) {
        w->codec->profile = AV_PROFILE_PRORES_4444;
    } else if (dnx) {
        if ((ret = av_opt_set(w->codec->priv_data, "profile", "dnxhr_hqx", 0)) < 0) goto failed;
    } else if (hevc) {
        w->codec->profile = AV_PROFILE_HEVC_MAIN_10;
        w->codec->max_b_frames = 0;
        w->codec->gop_size = 48;
        w->codec->codec_tag = MKTAG('h','v','c','1');
        if ((ret = av_opt_set(w->codec->priv_data, "profile", "main10", 0)) < 0) goto failed;
        if ((ret = av_opt_set(w->codec->priv_data, "preset", "medium", 0)) < 0) goto failed;
        if ((ret = av_opt_set(w->codec->priv_data, "crf", "16", 0)) < 0) goto failed;
        if ((ret = av_opt_set(w->codec->priv_data, "x265-params", "pools=none:frame-threads=2:wpp=0:bframes=0:log-level=error", 0)) < 0) goto failed;
    }
    if (w->format->oformat->flags & AVFMT_GLOBALHEADER) w->codec->flags |= AV_CODEC_FLAG_GLOBAL_HEADER;
    if ((ret = avcodec_open2(w->codec, encoder, NULL)) < 0) goto failed;
    if ((ret = avcodec_parameters_from_context(w->stream->codecpar, w->codec)) < 0) goto failed;
    if (color && (ret = eb_hdr_write(w->stream->codecpar, &color->hdr)) < 0) goto failed;
    w->stream->time_base = w->codec->time_base;
    w->stream->avg_frame_rate = w->codec->framerate;
    if (layout) {
        const AVCodec *pcm = avcodec_find_encoder(delivery_audio(encoding));
        if (!pcm) { ret = AVERROR_ENCODER_NOT_FOUND; goto failed; }
        w->audio = avcodec_alloc_context3(pcm);
        w->sound = avformat_new_stream(w->format, NULL);
        w->samples = av_frame_alloc();
        if (!w->audio || !w->sound || !w->samples) { ret = AVERROR(ENOMEM); goto failed; }
        w->audio->sample_fmt = aac ? AV_SAMPLE_FMT_FLTP : dnx ? AV_SAMPLE_FMT_S32 : AV_SAMPLE_FMT_FLT;
        if (dnx) w->audio->bits_per_raw_sample = 24;
        w->audio->sample_rate = rate;
        w->audio->time_base = (AVRational){1, rate};
        if ((ret = av_channel_layout_from_string(&w->audio->ch_layout, layout)) < 0) goto failed;
        if (aac) {
            if (w->audio->ch_layout.nb_channels > 2) { ret = AVERROR(EINVAL); goto failed; }
            w->audio->bit_rate = w->audio->ch_layout.nb_channels == 1 ? 192000 : 320000;
        }
        if (w->audio->ch_layout.order != AV_CHANNEL_ORDER_NATIVE) { ret = AVERROR(EINVAL); goto failed; }
        if (w->format->oformat->flags & AVFMT_GLOBALHEADER) w->audio->flags |= AV_CODEC_FLAG_GLOBAL_HEADER;
        if ((ret = avcodec_open2(w->audio, pcm, NULL)) < 0) goto failed;
        if (aac) {
            w->fifo = av_audio_fifo_alloc(AV_SAMPLE_FMT_FLTP, w->audio->ch_layout.nb_channels, 8192);
            if (!w->fifo) { ret = AVERROR(ENOMEM); goto failed; }
        }
        if ((ret = avcodec_parameters_from_context(w->sound->codecpar, w->audio)) < 0) goto failed;
        w->sound->time_base = w->audio->time_base;
    }
    if (layout && encoding != 5) {
        int64_t scale = (int64_t)num / av_gcd(num, rate) * rate;
        if (scale > INT32_MAX) { ret = AVERROR(EINVAL); goto failed; }
        if ((ret = av_dict_set_int(&options, "movie_timescale", scale, 0)) < 0) goto failed;
        if ((ret = av_dict_set_int(&options, "video_track_timescale", num, 0)) < 0) goto failed;
        if ((ret = av_dict_set(&options, "movflags", "+write_colr", 0)) < 0) goto failed;
    }
    ret = avformat_write_header(w->format, &options);
    av_dict_free(&options);
    if (ret < 0) goto failed;
    w->frame->format = w->codec->pix_fmt;
    w->frame->width = width;
    w->frame->height = height;
    w->frame->color_range = w->codec->color_range;
    w->frame->color_primaries = w->codec->color_primaries;
    w->frame->color_trc = w->codec->color_trc;
    w->frame->colorspace = w->codec->colorspace;
    w->frame->alpha_mode = w->codec->alpha_mode;
    if ((ret = av_frame_get_buffer(w->frame, 32)) < 0) goto failed;
    if (encoding == 1 || encoding == 2 || dnx || hevc) {
        w->conversion = sws_getContext(width, height, dnx || hevc ? AV_PIX_FMT_RGBA64LE : AV_PIX_FMT_RGBA, width, height,
            w->codec->pix_fmt, SWS_BICUBIC, NULL, NULL, NULL);
        if (!w->conversion) { ret = AVERROR(ENOMEM); goto failed; }
        const int *coefficients = sws_getCoefficients(w->codec->colorspace == AVCOL_SPC_BT2020_NCL ? SWS_CS_BT2020 : SWS_CS_ITU709);
        if ((ret = sws_setColorspaceDetails(w->conversion, coefficients, 1,
                coefficients, 0, 0, 1 << 16, 1 << 16)) < 0) goto failed;
    }
    *error = 0;
    return w;
failed:
    av_dict_free(&options);
    eb_writer_close(w);
    *error = ret;
    return NULL;
}

EBWriter *eb_writer_open(int descriptor, int width, int height, int num, int den, int *error) {
    return writer_open(descriptor, width, height, num, den, 0, NULL, 0, NULL, NULL, error);
}

EBWriter *eb_delivery_open_profile(int descriptor, int width, int height, int num, int den,
        int rate, const char *layout, int encoding, void *cancel, int *error) {
    return writer_open(descriptor, width, height, num, den, rate, layout, encoding, NULL, cancel, error);
}

EBWriter *eb_delivery_open_color_profile(int descriptor, int width, int height, int num, int den,
        int rate, const char *layout, int encoding, const EBColor *color, void *cancel, int *error) {
    if (!color) { *error = AVERROR(EINVAL); return NULL; }
    return writer_open(descriptor, width, height, num, den, rate, layout, encoding, color, cancel, error);
}

static int packets(EBWriter *w, AVCodecContext *codec, AVStream *stream) {
    for (;;) {
        if (w->cancel && eb_cancelled(w->cancel)) return AVERROR_EXIT;
        int ret = avcodec_receive_packet(codec, w->packet);
        if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF) return 0;
        if (ret < 0) return ret;
        if (w->sound_final && codec == w->audio && w->packet->pts >= 0
                && w->packet->pts + w->packet->duration > w->sound_end) {
            w->packet->duration = FFMAX(0, w->sound_end - w->packet->pts);
        }
        av_packet_rescale_ts(w->packet, codec->time_base, stream->time_base);
        w->packet->stream_index = stream->index;
        ret = av_interleaved_write_frame(w->format, w->packet);
        av_packet_unref(w->packet);
        if (ret < 0) return ret;
    }
}

int eb_writer_frame(EBWriter *w, const uint8_t *rgba, size_t length) {
    if (w->cancel && eb_cancelled(w->cancel)) return AVERROR_EXIT;
    int bytes = w->encoding >= 3 ? 8 : 4;
    if (length != (size_t)w->codec->width * w->codec->height * bytes) return AVERROR(EINVAL);
    if (w->encoding >= 4) for (size_t i = 6; i < length; i += 8)
        if (rgba[i] != 255 || rgba[i+1] != 255) return AVERROR(EINVAL);
    int ret = av_frame_make_writable(w->frame);
    if (ret < 0) return ret;
    if (w->conversion) {
        const uint8_t *input[] = {rgba};
        int stride[] = {w->codec->width * bytes};
        ret = sws_scale(w->conversion, input, stride, 0, w->codec->height,
            w->frame->data, w->frame->linesize);
        if (ret != w->codec->height) return ret < 0 ? ret : AVERROR(EINVAL);
    } else for (int y = 0; y < w->codec->height; y++) {
        uint8_t *row = w->frame->data[0] + y * w->frame->linesize[0];
        const uint8_t *source = rgba + (size_t)y * w->codec->width * bytes;
        if (w->encoding == 3) {
            for (int x = 0; x < w->codec->width * 4; x++) {
                row[x*2] = source[x*2+1]; row[x*2+1] = source[x*2];
            }
        } else if (w->codec->pix_fmt == AV_PIX_FMT_RGBA) {
            memcpy(row, source, (size_t)w->codec->width * 4);
        } else for (int x = 0; x < w->codec->width; x++) {
            row[x*4] = source[x*4+2]; row[x*4+1] = source[x*4+1];
            row[x*4+2] = source[x*4]; row[x*4+3] = source[x*4+3];
        }
    }
    w->frame->pts = w->frames++;
    w->frame->duration = 1;
    if ((ret = avcodec_send_frame(w->codec, w->frame)) < 0) return ret;
    return packets(w, w->codec, w->stream);
}

static int drain_sound(EBWriter *w, int final) {
    int size = w->audio->frame_size;
    while (av_audio_fifo_size(w->fifo) >= size || (final && av_audio_fifo_size(w->fifo))) {
        int count = FFMIN(size, av_audio_fifo_size(w->fifo));
        av_frame_unref(w->samples);
        w->samples->format = w->audio->sample_fmt;
        w->samples->sample_rate = w->audio->sample_rate;
        w->samples->nb_samples = count;
        int ret = av_channel_layout_copy(&w->samples->ch_layout, &w->audio->ch_layout);
        if (ret < 0) return ret;
        if ((ret = av_frame_get_buffer(w->samples, 0)) < 0) return ret;
        if (av_audio_fifo_read(w->fifo, (void **)w->samples->data, count) != count) return AVERROR(EIO);
        w->samples->pts = w->sound_frames;
        w->samples->duration = count;
        w->sound_frames += count;
        if ((ret = avcodec_send_frame(w->audio, w->samples)) < 0) return ret;
        if ((ret = packets(w, w->audio, w->sound)) < 0) return ret;
    }
    return 0;
}

int eb_delivery_sound(EBWriter *w, const float *samples, size_t length) {
    if (w->cancel && eb_cancelled(w->cancel)) return AVERROR_EXIT;
    if (!w->audio || !length || length % w->audio->ch_layout.nb_channels) return AVERROR(EINVAL);
    size_t count = length / w->audio->ch_layout.nb_channels;
    if (count > 4096 || w->sound_frames > INT64_MAX - (int64_t)count) return AVERROR(EINVAL);
    if (w->fifo) {
        int channels = w->audio->ch_layout.nb_channels;
        if (av_audio_fifo_space(w->fifo) < (int)count) return AVERROR(ENOMEM);
        av_frame_unref(w->samples);
        w->samples->format = AV_SAMPLE_FMT_FLTP;
        w->samples->nb_samples = count;
        int ret = av_channel_layout_copy(&w->samples->ch_layout, &w->audio->ch_layout);
        if (ret < 0) return ret;
        if ((ret = av_frame_get_buffer(w->samples, 0)) < 0) return ret;
        for (int channel = 0; channel < channels; channel++) {
            float *plane = (float *)w->samples->data[channel];
            for (size_t frame = 0; frame < count; frame++) plane[frame] = samples[frame * channels + channel];
        }
        if (av_audio_fifo_write(w->fifo, (void **)w->samples->data, count) != (int)count) return AVERROR(EIO);
        return drain_sound(w, 0);
    }
    av_frame_unref(w->samples);
    w->samples->format = w->audio->sample_fmt;
    w->samples->sample_rate = w->audio->sample_rate;
    w->samples->nb_samples = count;
    int ret = av_channel_layout_copy(&w->samples->ch_layout, &w->audio->ch_layout);
    if (ret < 0) return ret;
    if ((ret = av_frame_get_buffer(w->samples, 0)) < 0) return ret;
    if (w->encoding == 4 || w->encoding == 5) {
        int32_t *output = (int32_t *)w->samples->data[0];
        for (size_t i = 0; i < length; i++) {
            double scaled = (double)samples[i] * 8388608.0;
            if (!(samples[i] >= -1.0f && samples[i] <= 1.0f)) return AVERROR(EINVAL);
            int64_t quantized = (int64_t)(scaled + (scaled < 0 ? -0.5 : 0.5));
            quantized = FFMIN(8388607, FFMAX(-8388608, quantized));
            output[i] = (int32_t)(quantized * 256);
        }
    } else memcpy(w->samples->data[0], samples, length * sizeof(float));
    w->samples->pts = w->sound_frames;
    w->samples->duration = count;
    w->sound_frames += count;
    if ((ret = avcodec_send_frame(w->audio, w->samples)) < 0) return ret;
    return packets(w, w->audio, w->sound);
}

int eb_writer_finish(EBWriter *w) {
    int ret = avcodec_send_frame(w->codec, NULL);
    if (ret < 0) return ret;
    if ((ret = packets(w, w->codec, w->stream)) < 0) return ret;
    if (w->audio) {
        w->sound_final = 1;
        w->sound_end = w->sound_frames + (w->fifo ? av_audio_fifo_size(w->fifo) : 0);
        if (w->fifo && (ret = drain_sound(w, 1)) < 0) return ret;
        if ((ret = avcodec_send_frame(w->audio, NULL)) < 0) return ret;
        if ((ret = packets(w, w->audio, w->sound)) < 0) return ret;
    }
    if ((ret = av_write_trailer(w->format)) < 0) return ret;
    avio_flush(w->format->pb);
    if (w->format->pb->error < 0) return w->format->pb->error;
    return fflush(w->file) == 0 ? 0 : AVERROR(errno);
}

void eb_error(int code, char *message, size_t length) { av_strerror(code, message, length); }
const char *eb_version(void) { return av_version_info(); }
