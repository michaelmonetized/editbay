#include <stdint.h>
#include <stdlib.h>
#include <lcms2.h>

typedef struct EBCancel EBCancel;
int eb_cancelled(EBCancel *cancel);

typedef struct {
    cmsContext context;
    cmsHPROFILE input, output;
    cmsHTRANSFORM transform;
} EBColorTransform;

void eb_color_close(EBColorTransform *color) {
    if (!color) return;
    if (color->transform) cmsDeleteTransform(color->transform);
    if (color->input) cmsCloseProfile(color->input);
    if (color->output) cmsCloseProfile(color->output);
    if (color->context) cmsDeleteContext(color->context);
    free(color);
}

EBColorTransform *eb_color_open(const uint8_t *bytes, size_t length, int gamut, int premultiplied) {
    if (!bytes || length < 128 || length > 1024 * 1024) return NULL;
    EBColorTransform *color = calloc(1, sizeof(*color));
    if (!color) return NULL;
    color->context = cmsCreateContext(NULL, NULL);
    if (!color->context) goto failed;
    color->input = cmsOpenProfileFromMemTHR(color->context, bytes, (cmsUInt32Number)length);
    if (!color->input || cmsGetColorSpace(color->input) != cmsSigRgbData) goto failed;
    cmsCIExyY white = {0.3127, 0.3290, 1.0};
    cmsCIExyYTRIPLE primaries;
    if (gamut == 1) primaries = (cmsCIExyYTRIPLE){{0.64,0.33,1.0},{0.30,0.60,1.0},{0.15,0.06,1.0}};
    else if (gamut == 9) primaries = (cmsCIExyYTRIPLE){{0.708,0.292,1.0},{0.170,0.797,1.0},{0.131,0.046,1.0}};
    else if (gamut == 12) primaries = (cmsCIExyYTRIPLE){{0.680,0.320,1.0},{0.265,0.690,1.0},{0.150,0.060,1.0}};
    else goto failed;
    cmsToneCurve *linear = cmsBuildGamma(color->context, 1.0);
    if (!linear) goto failed;
    cmsToneCurve *curves[] = {linear, linear, linear};
    color->output = cmsCreateRGBProfileTHR(color->context, &white, &primaries, curves);
    cmsFreeToneCurve(linear);
    if (!color->output) goto failed;
    color->transform = cmsCreateTransformTHR(color->context, color->input,
        premultiplied ? TYPE_RGBA_FLT_PREMUL : TYPE_RGBA_FLT, color->output, TYPE_RGBA_FLT,
        INTENT_RELATIVE_COLORIMETRIC, cmsFLAGS_COPY_ALPHA | cmsFLAGS_NOOPTIMIZE | cmsFLAGS_NOCACHE);
    if (!color->transform) goto failed;
    return color;
failed:
    eb_color_close(color);
    return NULL;
}

int eb_color_apply(EBColorTransform *color, float *rgba, size_t pixels, EBCancel *cancel) {
    if (!color || !rgba || pixels > 8192u * 8192u) return -1;
    for (size_t first = 0; first < pixels; first += 16384) {
        if (eb_cancelled(cancel)) return -2;
        size_t count = pixels - first < 16384 ? pixels - first : 16384;
        cmsDoTransform(color->transform, rgba + first * 4, rgba + first * 4, (cmsUInt32Number)count);
    }
    return eb_cancelled(cancel) ? -2 : 0;
}
