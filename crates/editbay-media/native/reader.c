#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/channel_layout.h>
#include <libavutil/display.h>
#include <libavutil/pixdesc.h>
#include <libswscale/swscale.h>
#include <libswresample/swresample.h>

typedef struct EBCancel { atomic_int requested; } EBCancel;
_Static_assert(ATOMIC_INT_LOCK_FREE == 2, "Cancellation requires lock-free integer atomics");

EBCancel *eb_cancel_create(void) {
    EBCancel *cancel = calloc(1, sizeof(*cancel));
    if (cancel) atomic_init(&cancel->requested, 0);
    return cancel;
}

void eb_cancel_destroy(EBCancel *cancel) { free(cancel); }
void eb_cancel_request(EBCancel *cancel) { atomic_store(&cancel->requested, 1); }
int eb_cancelled(EBCancel *cancel) { return atomic_load(&cancel->requested); }

typedef struct {
    AVFormatContext *format;
    AVIOContext *io;
    int descriptor;
    int64_t position;
    EBCancel *cancel;
} EBInput;

static int interrupted(void *opaque) {
    EBInput *input = opaque;
    return eb_cancelled(input->cancel);
}

static int read_bytes(void *opaque, uint8_t *bytes, int length) {
    EBInput *input = opaque;
    if (interrupted(input)) return AVERROR_EXIT;
    ssize_t count;
    do { count = pread(input->descriptor, bytes, length, input->position); }
    while (count < 0 && errno == EINTR && !interrupted(input));
    if (count < 0) return AVERROR(errno);
    if (!count) return AVERROR_EOF;
    input->position += count;
    return (int)count;
}

static int64_t seek_input(void *opaque, int64_t offset, int whence) {
    EBInput *input = opaque;
    struct stat info;
    if (interrupted(input)) return AVERROR_EXIT;
    if (fstat(input->descriptor, &info)) return AVERROR(errno);
    if (whence == AVSEEK_SIZE) return info.st_size;
    whence &= ~AVSEEK_FORCE;
    int64_t base = whence == SEEK_SET ? 0 : whence == SEEK_CUR ? input->position :
        whence == SEEK_END ? info.st_size : -1;
    if (base < 0 || (offset > 0 && base > INT64_MAX - offset) ||
        (offset < 0 && offset < -base)) return AVERROR(EINVAL);
    input->position = base + offset;
    return input->position;
}

static int reject_secondary(AVFormatContext *context, AVIOContext **io,
    const char *url, int flags, AVDictionary **options) {
    (void)context; (void)io; (void)url; (void)flags; (void)options;
    return AVERROR(EACCES);
}

static void input_close(EBInput *input) {
    avformat_close_input(&input->format);
    if (input->io) {
        av_freep(&input->io->buffer);
        avio_context_free(&input->io);
    }
    if (input->descriptor >= 0) close(input->descriptor);
}

static int input_open(EBInput *input, int descriptor, const char *label, EBCancel *cancel) {
    struct stat info;
    input->descriptor = -1;
    input->cancel = cancel;
    if (eb_cancelled(cancel)) return AVERROR_EXIT;
    if (fstat(descriptor, &info) || !S_ISREG(info.st_mode)) return AVERROR(EINVAL);
    input->descriptor = fcntl(descriptor, F_DUPFD_CLOEXEC, 0);
    if (input->descriptor < 0) return AVERROR(errno);
    input->format = avformat_alloc_context();
    if (!input->format) return AVERROR(ENOMEM);
    unsigned char *buffer = av_malloc(32768);
    if (!buffer) return AVERROR(ENOMEM);
    input->io = avio_alloc_context(buffer, 32768, 0, input, read_bytes, NULL, seek_input);
    if (!input->io) { av_free(buffer); return AVERROR(ENOMEM); }
    input->format->pb = input->io;
    input->format->flags |= AVFMT_FLAG_CUSTOM_IO;
    input->format->opaque = input;
    input->format->io_open = reject_secondary;
    input->format->interrupt_callback = (AVIOInterruptCB){interrupted, input};
    input->format->max_streams = 256;
    input->format->max_index_size = 16 * 1024 * 1024;
    input->format->probesize = 8 * 1024 * 1024;
    input->format->max_analyze_duration = 5 * AV_TIME_BASE;
    AVDictionary *options = NULL;
    av_dict_set(&options, "protocol_whitelist", "file", 0);
    av_dict_set(&options, "format_whitelist", "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,avi,mpegts,mpeg,mp3,wav,flac,ogg,aac,aiff,image2pipe,png_pipe,jpeg_pipe,tiff_pipe,webp_pipe,exr_pipe,bmp_pipe,gif", 0);
    int ret = avformat_open_input(&input->format, label, NULL, &options);
    av_dict_free(&options);
    if (ret < 0) return ret;
    if (input->format->iformat->flags & AVFMT_NOFILE) return AVERROR(EACCES);
    unsigned int count = input->format->nb_streams;
    AVDictionary **decode_options = av_calloc(count ? count : 1, sizeof(*decode_options));
    if (!decode_options) return AVERROR(ENOMEM);
    for (unsigned int i = 0; i < count; i++) {
        av_dict_set(&decode_options[i], "max_pixels", "67108864", 0);
        av_dict_set(&decode_options[i], "threads", "2", 0);
    }
    ret = avformat_find_stream_info(input->format, decode_options);
    for (unsigned int i = 0; i < count; i++) av_dict_free(&decode_options[i]);
    av_free(decode_options);
    return ret;
}

typedef struct {
    int index, kind, width, height, rate_num, rate_den, aspect_num, aspect_den;
    int base_num, base_den, sample_rate, channels, channel_order, pixel_format;
    int color_primaries, color_transfer, color_matrix, color_range, alpha_mode, has_alpha;
    int disposition, decoder_available;
    int64_t start_tick, duration_ticks;
    double rotation;
} EBStream;

static void stream_info(AVFormatContext *format, int index, EBStream *info) {
    AVStream *stream = format->streams[index];
    AVCodecParameters *codec = stream->codecpar;
    memset(info, 0, sizeof(*info));
    info->index = index;
    info->kind = codec->codec_type;
    info->width = codec->width; info->height = codec->height;
    AVRational rate = av_guess_frame_rate(format, stream, NULL);
    info->rate_num = rate.num; info->rate_den = rate.den;
    AVRational aspect = av_guess_sample_aspect_ratio(format, stream, NULL);
    info->aspect_num = aspect.num; info->aspect_den = aspect.den;
    info->base_num = stream->time_base.num; info->base_den = stream->time_base.den;
    info->sample_rate = codec->sample_rate; info->channels = codec->ch_layout.nb_channels;
    info->channel_order = codec->ch_layout.order; info->pixel_format = codec->format;
    info->color_primaries = codec->color_primaries; info->color_transfer = codec->color_trc;
    info->color_matrix = codec->color_space; info->color_range = codec->color_range;
    info->alpha_mode = codec->alpha_mode;
    const AVPixFmtDescriptor *pixel = codec->codec_type == AVMEDIA_TYPE_VIDEO ? av_pix_fmt_desc_get(codec->format) : NULL;
    info->has_alpha = pixel && (pixel->flags & AV_PIX_FMT_FLAG_ALPHA);
    info->disposition = stream->disposition;
    info->decoder_available = avcodec_find_decoder(codec->codec_id) != NULL;
    info->start_tick = stream->start_time;
    info->duration_ticks = stream->duration;
    const AVPacketSideData *matrix = av_packet_side_data_get(codec->coded_side_data,
        codec->nb_coded_side_data, AV_PKT_DATA_DISPLAYMATRIX);
    if (matrix && matrix->size >= 9 * sizeof(int32_t))
        info->rotation = -av_display_rotation_get((const int32_t *)matrix->data);
}

EBInput *eb_probe_open(int descriptor, const char *label, EBCancel *cancel, int *error) {
    EBInput *input = calloc(1, sizeof(*input));
    if (!input) { *error = AVERROR(ENOMEM); return NULL; }
    *error = input_open(input, descriptor, label, cancel);
    if (*error < 0) { input_close(input); free(input); return NULL; }
    return input;
}

void eb_probe_close(EBInput *input) { if (input) { input_close(input); free(input); } }
int eb_probe_streams(EBInput *input) { return input->format->nb_streams; }

int eb_probe_info(EBInput *input, int index, EBStream *info, const char **codec) {
    if (index < 0 || index >= (int)input->format->nb_streams) return AVERROR(EINVAL);
    stream_info(input->format, index, info);
    *codec = avcodec_get_name(input->format->streams[index]->codecpar->codec_id);
    return 0;
}

int eb_probe_metadata(EBInput *input, int stream, int index, const char **key, const char **value) {
    if (stream < -1 || stream >= (int)input->format->nb_streams || index < 0)
        return AVERROR(EINVAL);
    AVDictionary *metadata = stream == -1 ? input->format->metadata :
        input->format->streams[stream]->metadata;
    const AVDictionaryEntry *entry = NULL;
    for (int i = 0; i <= index; i++) {
        entry = av_dict_get(metadata, "", entry, AV_DICT_IGNORE_SUFFIX);
        if (!entry) return 0;
    }
    *key = entry->key; *value = entry->value;
    return 1;
}

int eb_probe_channel(EBInput *input, int stream, int index, char *name, size_t capacity) {
    if (stream < 0 || stream >= (int)input->format->nb_streams) return AVERROR(EINVAL);
    AVChannelLayout *layout = &input->format->streams[stream]->codecpar->ch_layout;
    if (index < 0 || index >= layout->nb_channels) return AVERROR(EINVAL);
    enum AVChannel channel = av_channel_layout_channel_from_index(layout, index);
    if (layout->order == AV_CHANNEL_ORDER_CUSTOM && layout->u.map[index].name[0])
        return snprintf(name, capacity, "%s", layout->u.map[index].name);
    if (channel == AV_CHAN_NONE || channel == AV_CHAN_UNKNOWN || channel == AV_CHAN_UNUSED)
        return snprintf(name, capacity, "U%d", index);
    return av_channel_name(name, capacity, channel);
}

typedef struct {
    int width, height, rate_num, rate_den, sample_rate, channels;
    int64_t duration_ns;
    int color_primaries, color_transfer, color_matrix, color_range;
} EBInfo;

typedef struct {
    int64_t pts, duration, sample_start;
    int color_primaries, color_transfer, color_matrix, color_range, alpha_mode, has_alpha;
} EBFrame;

typedef struct {
    EBInput input;
    AVCodecContext *codec;
    AVPacket *packet;
    AVFrame *frame;
    struct SwsContext *scale;
    SwrContext *resample;
    int stream, audio, draining, sample_rate, channels;
    int64_t next_sample;
} EBReader;

void eb_reader_close(EBReader *reader) {
    if (!reader) return;
    sws_freeContext(reader->scale); swr_free(&reader->resample);
    av_frame_free(&reader->frame); av_packet_free(&reader->packet);
    avcodec_free_context(&reader->codec); input_close(&reader->input);
    free(reader);
}

EBReader *eb_reader_open(int descriptor, const char *label, int selected, int mode,
    EBCancel *cancel, EBInfo *info, EBStream *details, int *error) {
    EBReader *reader = calloc(1, sizeof(*reader));
    int ret = AVERROR(ENOMEM);
    if (!reader) { *error = ret; return NULL; }
    reader->input.descriptor = -1;
    reader->audio = mode != 0; reader->next_sample = AV_NOPTS_VALUE;
    if ((ret = input_open(&reader->input, descriptor, label, cancel)) < 0) goto failed;
    AVFormatContext *format = reader->input.format;
    enum AVMediaType kind = reader->audio ? AVMEDIA_TYPE_AUDIO : AVMEDIA_TYPE_VIDEO;
    if (selected == -1) {
        selected = av_find_best_stream(format, kind, -1, -1, NULL, 0);
        if (selected < 0) { ret = selected; goto failed; }
    }
    if (selected < 0 || selected >= (int)format->nb_streams ||
        format->streams[selected]->codecpar->codec_type != kind) { ret = AVERROR(EINVAL); goto failed; }
    reader->stream = selected;
    AVCodecParameters *parameters = format->streams[selected]->codecpar;
    const AVCodec *decoder = avcodec_find_decoder(parameters->codec_id);
    if (!decoder) { ret = AVERROR_DECODER_NOT_FOUND; goto failed; }
    reader->codec = avcodec_alloc_context3(decoder);
    if (!reader->codec) { ret = AVERROR(ENOMEM); goto failed; }
    if ((ret = avcodec_parameters_to_context(reader->codec, parameters)) < 0) goto failed;
    reader->codec->pkt_timebase = format->streams[selected]->time_base;
    reader->codec->thread_count = 2;
    reader->codec->max_pixels = 8192LL * 8192;
    if ((ret = avcodec_open2(reader->codec, decoder, NULL)) < 0) goto failed;
    reader->packet = av_packet_alloc(); reader->frame = av_frame_alloc();
    if (!reader->packet || !reader->frame) { ret = AVERROR(ENOMEM); goto failed; }
    stream_info(format, selected, details);
    memset(info, 0, sizeof(*info));
    info->duration_ns = format->duration == AV_NOPTS_VALUE ? -1 :
        av_rescale_q(format->duration, AV_TIME_BASE_Q, (AVRational){1,1000000000});
    info->color_primaries = details->color_primaries; info->color_transfer = details->color_transfer;
    info->color_matrix = details->color_matrix; info->color_range = details->color_range;
    if (reader->audio) {
        if (reader->codec->ch_layout.nb_channels < 1 || reader->codec->ch_layout.nb_channels > 64 ||
            reader->codec->sample_rate < 1 || reader->codec->sample_rate > 768000) {
            ret = AVERROR(EINVAL); goto failed;
        }
        reader->sample_rate = mode == 1 ? 48000 : reader->codec->sample_rate;
        reader->channels = mode == 1 ? 2 : reader->codec->ch_layout.nb_channels;
        info->sample_rate = reader->sample_rate; info->channels = reader->channels;
        AVChannelLayout input_layout = {0}, output_layout = {0};
        if (reader->codec->ch_layout.order == AV_CHANNEL_ORDER_UNSPEC)
            av_channel_layout_default(&input_layout, reader->codec->ch_layout.nb_channels);
        else if ((ret = av_channel_layout_copy(&input_layout, &reader->codec->ch_layout)) < 0) goto failed;
        if (mode == 1) av_channel_layout_default(&output_layout, 2);
        else ret = av_channel_layout_copy(&output_layout, &input_layout);
        if (ret >= 0) ret = swr_alloc_set_opts2(&reader->resample, &output_layout, AV_SAMPLE_FMT_FLT,
            reader->sample_rate, &input_layout, reader->codec->sample_fmt, reader->codec->sample_rate, 0, NULL);
        av_channel_layout_uninit(&input_layout); av_channel_layout_uninit(&output_layout);
        if (ret < 0 || (ret = swr_init(reader->resample)) < 0) goto failed;
    } else {
        info->width = reader->codec->width; info->height = reader->codec->height;
        info->rate_num = details->rate_num; info->rate_den = details->rate_den;
        if (info->width <= 0 || info->height <= 0 || info->width > 8192 || info->height > 8192) {
            ret = AVERROR(EINVAL); goto failed;
        }
    }
    *error = 0;
    return reader;
failed:
    eb_reader_close(reader); *error = ret; return NULL;
}

static int receive(EBReader *reader) {
    for (;;) {
        if (interrupted(&reader->input)) return AVERROR_EXIT;
        int ret = avcodec_receive_frame(reader->codec, reader->frame);
        if (ret != AVERROR(EAGAIN)) return ret;
        if (reader->draining) return AVERROR_EOF;
        do {
            if (interrupted(&reader->input)) return AVERROR_EXIT;
            ret = av_read_frame(reader->input.format, reader->packet);
            if (ret < 0 || reader->packet->stream_index == reader->stream) break;
            av_packet_unref(reader->packet);
        } while (1);
        if (ret == AVERROR_EOF) {
            reader->draining = 1; ret = avcodec_send_packet(reader->codec, NULL);
        } else if (ret >= 0) {
            ret = avcodec_send_packet(reader->codec, reader->packet);
            av_packet_unref(reader->packet);
        }
        if (ret < 0 && ret != AVERROR_EOF) return ret;
    }
}

int eb_reader_seek(EBReader *reader, int64_t tick) {
    if (interrupted(&reader->input)) return AVERROR_EXIT;
    int ret = avformat_seek_file(reader->input.format, reader->stream, INT64_MIN, tick, tick, 0);
    if (ret < 0) return ret;
    avcodec_flush_buffers(reader->codec); av_packet_unref(reader->packet); av_frame_unref(reader->frame);
    reader->draining = 0; reader->next_sample = AV_NOPTS_VALUE;
    if (reader->resample) {
        swr_close(reader->resample);
        ret = swr_init(reader->resample);
    }
    return ret;
}

static int reader_next(EBReader *reader, uint8_t *output, size_t capacity, EBFrame *details,
    int selected, int64_t tick) {
    memset(details, 0, sizeof(*details));
    details->pts = details->sample_start = AV_NOPTS_VALUE;
next_frame:
    int ret = receive(reader);
    if (ret == AVERROR_EOF) {
        if (!reader->audio) return 0;
        uint8_t *planes[] = {output};
        int count = swr_convert(reader->resample, planes, (int)(capacity / (4 * reader->channels)), NULL, 0);
        if (count < 0) return count;
        details->sample_start = reader->next_sample;
        if (reader->next_sample != AV_NOPTS_VALUE) reader->next_sample += count;
        return count * 4 * reader->channels;
    }
    if (ret < 0) return ret;
    AVFrame *frame = reader->frame;
    details->pts = frame->best_effort_timestamp; details->duration = frame->duration;
    details->color_primaries = frame->color_primaries; details->color_transfer = frame->color_trc;
    details->color_matrix = frame->colorspace; details->color_range = frame->color_range;
    details->alpha_mode = frame->alpha_mode;
    const AVPixFmtDescriptor *pixel = av_pix_fmt_desc_get(frame->format);
    details->has_alpha = pixel && (pixel->flags & AV_PIX_FMT_FLAG_ALPHA);
    const AVCodecParameters *parameters = reader->input.format->streams[reader->stream]->codecpar;
    if (details->color_primaries == AVCOL_PRI_UNSPECIFIED) details->color_primaries = parameters->color_primaries;
    if (details->color_transfer == AVCOL_TRC_UNSPECIFIED) details->color_transfer = parameters->color_trc;
    if (details->color_matrix == AVCOL_SPC_UNSPECIFIED) details->color_matrix = parameters->color_space;
    if (details->color_range == AVCOL_RANGE_UNSPECIFIED) details->color_range = parameters->color_range;
    if (reader->audio) {
        if (frame->sample_rate != reader->codec->sample_rate ||
            frame->format != reader->codec->sample_fmt ||
            frame->ch_layout.nb_channels != reader->codec->ch_layout.nb_channels ||
            (frame->ch_layout.order != AV_CHANNEL_ORDER_UNSPEC &&
            reader->codec->ch_layout.order != AV_CHANNEL_ORDER_UNSPEC &&
            av_channel_layout_compare(&frame->ch_layout, &reader->codec->ch_layout))) return AVERROR(EINVAL);
        int count = swr_get_out_samples(reader->resample, frame->nb_samples);
        if (count < 0 || (size_t)count * 4 * reader->channels > capacity) return AVERROR(ENOBUFS);
        if (details->pts != AV_NOPTS_VALUE) {
            AVRational base = reader->input.format->streams[reader->stream]->time_base;
            int64_t presented = av_rescale_q(details->pts, base, (AVRational){1,reader->sample_rate}) -
                swr_get_delay(reader->resample, reader->sample_rate);
            int64_t resolution = av_rescale_q_rnd(1, base, (AVRational){1,reader->sample_rate}, AV_ROUND_UP);
            int64_t tolerance = resolution > 1 ? resolution + 1 : 0;
            __int128 difference = (__int128)presented - reader->next_sample;
            if (reader->next_sample == AV_NOPTS_VALUE || difference < -tolerance || difference > tolerance)
                reader->next_sample = presented;
        }
        uint8_t *planes[] = {output};
        count = swr_convert(reader->resample, planes, count, (const uint8_t **)frame->extended_data, frame->nb_samples);
        if (count < 0) return count;
        if (!count) goto next_frame;
        details->sample_start = reader->next_sample;
        if (reader->next_sample != AV_NOPTS_VALUE) reader->next_sample += count;
        return count * 4 * reader->channels;
    }
    int width = reader->codec->width, height = reader->codec->height;
    if (frame->width != width || frame->height != height) return AVERROR(EINVAL);
    if (selected) {
        if (details->pts == AV_NOPTS_VALUE || details->pts > tick) return AVERROR(EINVAL);
        if (details->pts < tick) goto next_frame;
    }
    if (!output) return 1;
    if (capacity < (size_t)width * height * 4) return AVERROR(ENOBUFS);
    reader->scale = sws_getCachedContext(reader->scale, width, height, frame->format, width, height,
        AV_PIX_FMT_RGBA, SWS_BILINEAR, NULL, NULL, NULL);
    if (!reader->scale) return AVERROR(ENOMEM);
    int matrix = details->color_matrix == AVCOL_SPC_BT709 ? SWS_CS_ITU709 :
        details->color_matrix == AVCOL_SPC_BT2020_NCL ? SWS_CS_BT2020 :
        details->color_matrix == AVCOL_SPC_FCC ? SWS_CS_FCC :
        details->color_matrix == AVCOL_SPC_SMPTE240M ? SWS_CS_SMPTE240M :
        details->color_matrix == AVCOL_SPC_UNSPECIFIED && (width >= 1280 || height >= 720) ? SWS_CS_ITU709 : SWS_CS_ITU601;
    const int *coefficients = sws_getCoefficients(matrix);
    ret = sws_setColorspaceDetails(reader->scale, coefficients, details->color_range == AVCOL_RANGE_JPEG,
        coefficients, 1, 0, 1<<16, 1<<16);
    if (ret < 0) return ret;
    uint8_t *planes[] = {output}; int strides[] = {width * 4};
    ret = sws_scale(reader->scale, (const uint8_t * const *)frame->data, frame->linesize, 0, height, planes, strides);
    if (interrupted(&reader->input)) return AVERROR_EXIT;
    return ret < 0 ? ret : width * height * 4;
}

int eb_reader_next(EBReader *reader, uint8_t *output, size_t capacity, EBFrame *details) {
    return reader_next(reader, output, capacity, details, 0, 0);
}

int eb_reader_picture_at(EBReader *reader, int64_t tick, uint8_t *output,
    size_t capacity, EBFrame *details) {
    if (reader->audio || !output) return AVERROR(EINVAL);
    int ret = eb_reader_seek(reader, tick);
    if (ret < 0) return ret;
    return reader_next(reader, output, capacity, details, 1, tick);
}
