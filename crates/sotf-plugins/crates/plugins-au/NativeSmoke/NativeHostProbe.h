#import <AudioToolbox/AUAudioUnitImplementation.h>
#include <stdint.h>

#define SOTF_NATIVE_AUTOMATION_NONE 0
#define SOTF_NATIVE_AUTOMATION_POINT 1
#define SOTF_NATIVE_AUTOMATION_RAMP 2

OSStatus sotf_test_native_render(
    AUAudioUnit *unit,
    AudioBufferList *input,
    AudioBufferList *output,
    AUAudioFrameCount frames,
    AUParameterAddress parameter,
    int automated,
    uint64_t *allocations,
    uint64_t *frees
);
