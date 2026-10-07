#define _GNU_SOURCE
#include <pipewire/pipewire.h>
#include <spa/param/audio/format-utils.h>
#include <spa/node/io.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

typedef void (*eb_render)(void *, float *, size_t, uint64_t, uint64_t, const uint64_t *);
struct eb_pw {
    struct pw_thread_loop *loop;
    struct pw_context *context;
    struct pw_core *core;
    struct pw_stream *stream;
    struct spa_hook listener;
    eb_render render;
    void *user;
    uint32_t rate, channels;
    atomic_uint frames;
    atomic_int state, error;
    atomic_bool closing;
    _Atomic(struct spa_io_clock *) clock;
    _Atomic(struct spa_io_position *) position;
    uint64_t calls, misses, previous_ticks, maximum_step;
    bool observed_ticks;
};

/* Report negotiated state without borrowing the real-time callback's Rust state.
 * data is the native owner; state changes wake bounded setup waits. */
static void state_changed(void *data, enum pw_stream_state old,
                          enum pw_stream_state state, const char *message) {
    (void)old; (void)message;
    struct eb_pw *output = data;
    atomic_store(&output->state, state);
    if (state == PW_STREAM_STATE_ERROR) atomic_store(&output->error, EIO);
    pw_thread_loop_signal(output->loop, false);
}

/* Validate the exact native float layout before any source samples are consumed.
 * data owns the stream and param declares its negotiated format. */
static void param_changed(void *data, uint32_t id, const struct spa_pod *param) {
    struct eb_pw *output = data;
    if (!param || id != SPA_PARAM_Format) return;
    struct spa_audio_info_raw info = {0};
    if (spa_format_audio_raw_parse(param, &info) < 0 || info.format != SPA_AUDIO_FORMAT_F32
        || info.rate != output->rate || info.channels != output->channels)
        atomic_store(&output->error, EINVAL);
}

/* Retain the native per-node clock published by PipeWire's data loop.
 * data owns the stream; area remains native-owned until the next IO change. */
static void io_changed(void *data, uint32_t id, void *area, uint32_t size) {
    struct eb_pw *output = data;
    if (id == SPA_IO_Clock)
        atomic_store(&output->clock, size >= sizeof(struct spa_io_clock) ? area : NULL);
    if (id == SPA_IO_Position)
        atomic_store(&output->position, size >= sizeof(struct spa_io_position) ? area : NULL);
}

/* Fill one device-owned buffer from the bounded pre-rendered Rust sound ring.
 * data is the native owner; the callback allocates nothing and publishes only
 * exact frame geometry and backend-derived timestamps to Rust. */
static void process(void *data) {
    struct eb_pw *output = data;
    output->calls++;
    struct spa_io_position *position = atomic_load(&output->position);
    struct spa_io_clock *clock = atomic_load(&output->clock);
    if (position) clock = &position->clock;
    struct pw_buffer *buffer = pw_stream_dequeue_buffer(output->stream);
    if (!buffer) { output->misses++; return; }
    buffer->size = 0;
    struct spa_buffer *spa = buffer->buffer;
    if (!spa || spa->n_datas != 1) { atomic_store(&output->error, EINVAL); goto queue; }
    struct spa_data *samples = &spa->datas[0];
    if (!samples->data || !samples->chunk) { atomic_store(&output->error, EINVAL); goto queue; }
    size_t stride = output->channels * sizeof(float);
    size_t capacity = samples->maxsize / stride;
    size_t frames = buffer->requested ? buffer->requested : capacity;
    if (!frames || frames > capacity || frames > 16384) {
        samples->chunk->size = 0; atomic_store(&output->error, EOVERFLOW); goto queue;
    }
    memset(samples->data, 0, frames * stride);
    struct pw_time time = {0};
    if (!atomic_load(&output->closing) && !atomic_load(&output->error)) {
        int result = pw_stream_get_time_n(output->stream, &time, sizeof(time));
        uint64_t callback = pw_stream_get_nsec(output->stream);
        if (result < 0 || time.now <= 0 || !time.rate.num || !time.rate.denom) {
            atomic_store(&output->error, EIO);
        } else {
            if (output->observed_ticks && time.ticks >= output->previous_ticks) {
                uint64_t step = time.ticks - output->previous_ticks;
                if (step > output->maximum_step) output->maximum_step = step;
            }
            output->previous_ticks = time.ticks;
            output->observed_ticks = true;
            __uint128_t graph = (__uint128_t)(time.delay > 0 ? time.delay : 0)
                * time.rate.num * 1000000000 / time.rate.denom;
            __uint128_t queued = ((__uint128_t)time.queued + time.buffered)
                * 1000000000 / output->rate;
            __uint128_t presentation = (__uint128_t)time.now + graph + queued;
            if (presentation > UINT64_MAX || presentation < callback)
                atomic_store(&output->error, EOVERFLOW);
            else {
                uint64_t diagnostic[17] = { output->calls, output->misses, time.ticks,
                    output->maximum_step, clock ? clock->flags : 0,
                    clock ? clock->duration : 0, clock ? clock->xrun : 0,
                    callback, (uint64_t)time.now, (uint64_t)time.delay,
                    time.queued, time.buffered, time.rate.num, time.rate.denom,
                    clock ? clock->id : UINT32_MAX, time.queued_buffers, time.avail_buffers };
                output->render(output->user, samples->data, frames * output->channels,
                               callback, (uint64_t)presentation, diagnostic);
            }
        }
    }
    samples->chunk->offset = 0;
    samples->chunk->stride = (int32_t)stride;
    samples->chunk->size = (uint32_t)(frames * stride);
    buffer->size = frames;
    atomic_store(&output->frames, (unsigned)frames);
queue:
    if (pw_stream_queue_buffer(output->stream, buffer) < 0) atomic_store(&output->error, EIO);
}

static const struct pw_stream_events events = {
    PW_VERSION_STREAM_EVENTS, .state_changed = state_changed,
    .param_changed = param_changed, .io_changed = io_changed, .process = process,
};

/* Retire both PipeWire loops before releasing the callback's borrowed owner.
 * output is the native handle; returns only after native teardown. */
void eb_pw_close(struct eb_pw *output) {
    if (!output) return;
    atomic_store(&output->closing, true);
    if (output->loop) pw_thread_loop_stop(output->loop);
    if (output->stream) pw_stream_destroy(output->stream);
    if (output->core) pw_core_disconnect(output->core);
    if (output->context) pw_context_destroy(output->context);
    if (output->loop) pw_thread_loop_destroy(output->loop);
    free(output);
}

/* Connect an inactive exact float stream with a true real-time process callback.
 * rate, channels and quantum declare its layout; render/user own the Rust ring.
 * Returns a native handle after bounded negotiation, or null with errno. */
struct eb_pw *eb_pw_open(uint32_t rate, uint32_t channels, uint32_t quantum,
                        eb_render render, void *user) {
    if (!rate || !channels || channels > 8 || !quantum || quantum > 16384 || !render || !user) {
        errno = EINVAL; return NULL;
    }
    pw_init(NULL, NULL);
    struct eb_pw *output = calloc(1, sizeof(*output));
    if (!output) return NULL;
    output->rate = rate; output->channels = channels; output->render = render; output->user = user;
    atomic_init(&output->frames, quantum); atomic_init(&output->state, PW_STREAM_STATE_UNCONNECTED);
    atomic_init(&output->error, 0); atomic_init(&output->closing, false);
    atomic_init(&output->clock, NULL);
    atomic_init(&output->position, NULL);
    output->loop = pw_thread_loop_new("editbay-device", NULL);
    if (!output->loop) goto fail;
    output->context = pw_context_new(pw_thread_loop_get_loop(output->loop), NULL, 0);
    if (!output->context) goto fail;
    output->core = pw_context_connect(output->context, NULL, 0);
    if (!output->core) goto fail;
    struct pw_properties *props = pw_properties_new(
        PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY, "Playback",
        PW_KEY_MEDIA_ROLE, "Production", PW_KEY_NODE_NAME, "EditBay", NULL);
    if (!props) goto fail;
    pw_properties_setf(props, PW_KEY_NODE_LATENCY, "%u/%u", quantum, rate);
    pw_properties_setf(props, PW_KEY_NODE_FORCE_QUANTUM, "%u", quantum);
    pw_properties_set(props, PW_KEY_NODE_LOCK_QUANTUM, "true");
    output->stream = pw_stream_new(output->core, "EditBay playback", props);
    if (!output->stream) goto fail;
    pw_stream_add_listener(output->stream, &output->listener, &events, output);
    uint8_t storage[1024];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    struct spa_audio_info_raw info = { .format = SPA_AUDIO_FORMAT_F32, .rate = rate, .channels = channels };
    if (channels == 1) info.position[0] = SPA_AUDIO_CHANNEL_MONO;
    else if (channels == 2) { info.position[0] = SPA_AUDIO_CHANNEL_FL; info.position[1] = SPA_AUDIO_CHANNEL_FR; }
    else { errno = EINVAL; goto fail; }
    const struct spa_pod *params[] = { spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &info) };
    int result = pw_stream_connect(output->stream, PW_DIRECTION_OUTPUT, PW_ID_ANY,
        PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_INACTIVE | PW_STREAM_FLAG_RT_PROCESS,
        params, 1);
    if (result < 0) { errno = -result; goto fail; }
    result = pw_thread_loop_start(output->loop);
    if (result < 0) { errno = -result; goto fail; }
    pw_thread_loop_lock(output->loop);
    for (int waits = 0; atomic_load(&output->state) < PW_STREAM_STATE_PAUSED && !atomic_load(&output->error); ++waits) {
        if (waits >= 5 || pw_thread_loop_timed_wait(output->loop, 1) < 0) {
            atomic_store(&output->error, ETIMEDOUT); break;
        }
    }
    int error = atomic_load(&output->error);
    pw_thread_loop_unlock(output->loop);
    if (error) { errno = error; goto fail; }
    return output;
fail:
    { int error = errno ? errno : EIO; eb_pw_close(output); errno = error; return NULL; }
}

/* Change stream activity on its serialized control loop.
 * output owns the stream and active selects playback; returns a native error. */
int eb_pw_active(struct eb_pw *output, int active) {
    pw_thread_loop_lock(output->loop);
    int result = pw_stream_set_active(output->stream, active != 0);
    pw_thread_loop_unlock(output->loop);
    return result;
}

/* Inspect native failure and callback geometry without locking either loop.
 * output owns the stream; returns its first native error or latest frame count. */
int eb_pw_error(struct eb_pw *output) { return atomic_load(&output->error); }
unsigned eb_pw_frames(struct eb_pw *output) { return atomic_load(&output->frames); }
