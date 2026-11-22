#import "NativeHostProbe.h"
#include <malloc/malloc.h>
#include <string.h>

void sotf_malloc_counter_begin(void);
void sotf_malloc_counter_end(uint64_t *, uint64_t *);

void sotf_malloc_counter_self_test(uint64_t *allocations, uint64_t *frees) {
    malloc_zone_t *zone = malloc_default_zone();
    void *(*volatile allocate)(size_t) = malloc;
    void (*volatile release)(void *) = free;
    sotf_malloc_counter_begin();
    void *p = allocate(123);
    release(p);
    p = calloc(2, 64);
    p = realloc(p, 256);
    free(p);
    p = NULL;
    if (posix_memalign(&p, 64, 128) == 0) free(p);
    p = malloc_zone_malloc(zone, 129);
    p = malloc_zone_realloc(zone, p, 257);
    malloc_zone_free(zone, p);
    p = malloc_zone_calloc(zone, 2, 64);
    malloc_zone_free(zone, p);
    p = malloc_zone_memalign(zone, 64, 128);
    malloc_zone_free(zone, p);
    p = malloc_zone_valloc(zone, 128);
    malloc_zone_free(zone, p);
    sotf_malloc_counter_end(allocations, frees);
}

OSStatus sotf_test_native_render(
    AUAudioUnit *unit,
    AudioBufferList *input,
    AudioBufferList *output,
    AUAudioFrameCount frames,
    AUParameterAddress parameter,
    int automated,
    uint64_t *allocations,
    uint64_t *frees
) {
    AURenderPullInputBlock __unsafe_unretained pull = ^AUAudioUnitStatus(
        AudioUnitRenderActionFlags *flags,
        const AudioTimeStamp *time,
        AUAudioFrameCount requested,
        NSInteger bus,
        AudioBufferList *destination
    ) {
        (void)flags; (void)time; (void)bus;
        if (requested != frames || input->mNumberBuffers != destination->mNumberBuffers) {
            return -9997;
        }
        for (UInt32 i = 0; i < input->mNumberBuffers; i++) {
            AudioBuffer *source = &input->mBuffers[i];
            AudioBuffer *target = &destination->mBuffers[i];
            size_t bytes = (size_t)requested * source->mNumberChannels * sizeof(float);
            if (!source->mData || !target->mData || source->mDataByteSize < bytes) {
                return -9996;
            }
            memcpy(target->mData, source->mData, bytes);
            target->mDataByteSize = (UInt32)bytes;
        }
        return noErr;
    };
    id copy = [pull copy];
    if ((__bridge void *)copy == (__bridge void *)pull) return -9999;
    copy = nil;

    AUInternalRenderBlock render = unit.internalRenderBlock;
    AudioUnitRenderActionFlags flags = 0;
    AudioTimeStamp time = {0};
    time.mFlags = kAudioTimeStampSampleTimeValid;
    AURenderEvent event = {0};
    event.parameter.eventType = automated == SOTF_NATIVE_AUTOMATION_RAMP
        ? AURenderEventParameterRamp : AURenderEventParameter;
    event.parameter.parameterAddress = parameter;
    event.parameter.rampDurationSampleFrames = frames * 2 + 17;
    OSStatus status = noErr;
    for (int i = -256; i < 4096; i++) {
        if (i == 0) sotf_malloc_counter_begin();
        time.mSampleTime = (double)((i + 256) * frames);
        event.parameter.eventSampleTime = (AUEventSampleTime)time.mSampleTime
            + (automated == SOTF_NATIVE_AUTOMATION_RAMP ? frames / 2 : 0);
        event.parameter.value = (automated == SOTF_NATIVE_AUTOMATION_RAMP ? i / 4 : i) % 2
            ? 0.5f : 1.5f;
        const AURenderEvent *events = automated == SOTF_NATIVE_AUTOMATION_NONE
            || (automated == SOTF_NATIVE_AUTOMATION_RAMP && i % 4 != 0) ? NULL : &event;
        status = render(&flags, &time, frames, 0, output, events, pull);
        if (status != noErr) {
            if (i >= 0) sotf_malloc_counter_end(allocations, frees);
            return status;
        }
    }
    sotf_malloc_counter_end(allocations, frees);
    return status;
}
