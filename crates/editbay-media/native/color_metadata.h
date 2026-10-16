#ifndef EDITBAY_COLOR_METADATA_H
#define EDITBAY_COLOR_METADATA_H
#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavutil/mastering_display_metadata.h>
#include <libavutil/mathematics.h>
#include <libavutil/mem.h>

typedef struct {
    int mastering, light;
    uint32_t primaries[3][2], white[2], minimum, maximum, cll, fall;
} EBHdr;
typedef struct { int primaries, transfer; EBHdr hdr; } EBColor;

static inline int eb_hdr_units(AVRational value, int scale, uint32_t *output) {
    if (value.num < 0 || value.den <= 0) return AVERROR(EINVAL);
    int64_t result = av_rescale_rnd(value.num, scale, value.den, AV_ROUND_NEAR_INF);
    if (result < 0 || result > UINT32_MAX) return AVERROR(EINVAL);
    *output = (uint32_t)result;
    return 0;
}

static inline int eb_hdr_read(const AVFrame *frame, const AVCodecParameters *parameters, EBHdr *output) {
    memset(output, 0, sizeof(*output));
    const AVFrameSideData *fm = frame ? av_frame_get_side_data(frame, AV_FRAME_DATA_MASTERING_DISPLAY_METADATA) : NULL;
    const AVFrameSideData *fl = frame ? av_frame_get_side_data(frame, AV_FRAME_DATA_CONTENT_LIGHT_LEVEL) : NULL;
    const AVPacketSideData *pm = av_packet_side_data_get(parameters->coded_side_data,
        parameters->nb_coded_side_data, AV_PKT_DATA_MASTERING_DISPLAY_METADATA);
    const AVPacketSideData *pl = av_packet_side_data_get(parameters->coded_side_data,
        parameters->nb_coded_side_data, AV_PKT_DATA_CONTENT_LIGHT_LEVEL);
    const uint8_t *mastering = fm ? fm->data : pm ? pm->data : NULL;
    size_t mastering_size = fm ? fm->size : pm ? pm->size : 0;
    const uint8_t *light = fl ? fl->data : pl ? pl->data : NULL;
    size_t light_size = fl ? fl->size : pl ? pl->size : 0;
    if (fm || pm) {
        if (!mastering || mastering_size < sizeof(AVMasteringDisplayMetadata)) return AVERROR(EINVAL);
        const AVMasteringDisplayMetadata *metadata = (const AVMasteringDisplayMetadata *)mastering;
        if (!metadata->has_primaries || !metadata->has_luminance) return AVERROR(EINVAL);
        output->mastering = 1;
        for (int p = 0; p < 3; p++) for (int xy = 0; xy < 2; xy++)
            if (eb_hdr_units(metadata->display_primaries[p][xy], 50000, &output->primaries[p][xy]) < 0) return AVERROR(EINVAL);
        for (int xy = 0; xy < 2; xy++)
            if (eb_hdr_units(metadata->white_point[xy], 50000, &output->white[xy]) < 0) return AVERROR(EINVAL);
        if (eb_hdr_units(metadata->min_luminance, 10000, &output->minimum) < 0 ||
            eb_hdr_units(metadata->max_luminance, 10000, &output->maximum) < 0) return AVERROR(EINVAL);
    }
    if (fl || pl) {
        if (!light || light_size < sizeof(AVContentLightMetadata)) return AVERROR(EINVAL);
        const AVContentLightMetadata *metadata = (const AVContentLightMetadata *)light;
        output->light = 1; output->cll = metadata->MaxCLL; output->fall = metadata->MaxFALL;
    }
    return 0;
}

static inline int eb_hdr_write(AVCodecParameters *parameters, const EBHdr *input) {
    if (input->mastering) {
        size_t size;
        AVMasteringDisplayMetadata *metadata = av_mastering_display_metadata_alloc_size(&size);
        if (!metadata) return AVERROR(ENOMEM);
        metadata->has_primaries = metadata->has_luminance = 1;
        for (int p = 0; p < 3; p++) for (int xy = 0; xy < 2; xy++)
            metadata->display_primaries[p][xy] = (AVRational){input->primaries[p][xy], 50000};
        for (int xy = 0; xy < 2; xy++) metadata->white_point[xy] = (AVRational){input->white[xy], 50000};
        metadata->min_luminance = (AVRational){input->minimum, 10000};
        metadata->max_luminance = (AVRational){input->maximum, 10000};
        if (!av_packet_side_data_add(&parameters->coded_side_data, &parameters->nb_coded_side_data,
            AV_PKT_DATA_MASTERING_DISPLAY_METADATA, (uint8_t *)metadata, size, 0)) {
            av_free(metadata); return AVERROR(ENOMEM);
        }
    }
    if (input->light) {
        size_t size;
        AVContentLightMetadata *metadata = av_content_light_metadata_alloc(&size);
        if (!metadata) return AVERROR(ENOMEM);
        metadata->MaxCLL = input->cll; metadata->MaxFALL = input->fall;
        if (!av_packet_side_data_add(&parameters->coded_side_data, &parameters->nb_coded_side_data,
            AV_PKT_DATA_CONTENT_LIGHT_LEVEL, (uint8_t *)metadata, size, 0)) {
            av_free(metadata); return AVERROR(ENOMEM);
        }
    }
    return 0;
}
#endif
