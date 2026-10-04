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
#include <libswscale/swscale.h>
#include <libswresample/swresample.h>


typedef struct {
    AVFormatContext *format;
    AVCodecContext *codec;
    AVStream *stream;
    AVPacket *packet;
    AVFrame *frame;
    int64_t frames;
    FILE *file;
} EBWriter;

void eb_writer_close(EBWriter *w) {
    if (!w) return;
    av_frame_free(&w->frame);
    av_packet_free(&w->packet);
    avcodec_free_context(&w->codec);
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
    FILE *file = opaque;
    return fwrite(bytes, 1, length, file) == (size_t)length ? length : AVERROR(EIO);
}

static int64_t seek_bytes(void *opaque, int64_t offset, int whence) {
    FILE *file = opaque;
    if (whence == AVSEEK_SIZE) {
        struct stat info;
        return fstat(fileno(file), &info) == 0 ? info.st_size : AVERROR(errno);
    }
    if (fseeko(file, offset, whence & ~AVSEEK_FORCE) != 0) return AVERROR(errno);
    return ftello(file);
}

EBWriter *eb_writer_open(int descriptor, int width, int height, int num, int den, int *error) {
    EBWriter *w = calloc(1, sizeof(*w));
    int ret = AVERROR(ENOMEM);
    if (!w) goto failed;
    struct stat file_info;
    if (fstat(descriptor, &file_info) != 0 || !S_ISREG(file_info.st_mode) || file_info.st_size != 0) {
        ret = AVERROR(EINVAL); goto failed;
    }
    int owned = fcntl(descriptor, F_DUPFD_CLOEXEC, 0);
    if (owned < 0) { ret = AVERROR(errno); goto failed; }
    w->file = fdopen(owned, "wb");
    if (!w->file) { close(owned); ret = AVERROR(errno); goto failed; }
    if ((ret = avformat_alloc_output_context2(&w->format, NULL, "matroska", NULL)) < 0) goto failed;
    unsigned char *buffer = av_malloc(32768);
    if (!buffer) { ret = AVERROR(ENOMEM); goto failed; }
    w->format->pb = avio_alloc_context(buffer, 32768, 1, w->file, NULL, write_bytes, seek_bytes);
    if (!w->format->pb) { av_free(buffer); ret = AVERROR(ENOMEM); goto failed; }
    w->format->flags |= AVFMT_FLAG_CUSTOM_IO;
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
    if ((ret = av_write_trailer(w->format)) < 0) return ret;
    avio_flush(w->format->pb);
    if (w->format->pb->error < 0) return w->format->pb->error;
    return fflush(w->file) == 0 ? 0 : AVERROR(errno);
}

void eb_error(int code, char *message, size_t length) { av_strerror(code, message, length); }
const char *eb_version(void) { return av_version_info(); }
