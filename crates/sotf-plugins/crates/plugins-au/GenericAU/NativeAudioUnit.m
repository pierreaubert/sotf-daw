#import "NativeAudioUnit.h"

@implementation SOTFNativeAudioUnit
- (id<SOTFRenderContext>)makeSOTFRenderContext {
    [NSException raise:NSInternalInconsistencyException format:@"Subclasses must implement makeSOTFRenderContext"];
    __builtin_unreachable();
}

- (AUInternalRenderBlock)internalRenderBlock {
    // The returned block owns the context for as long as the host retains it.
    id<SOTFRenderContext> context = [self makeSOTFRenderContext];
    return ^AUAudioUnitStatus(
        AudioUnitRenderActionFlags *flags,
        const AudioTimeStamp *time,
        AUAudioFrameCount frames,
        NSInteger bus,
        AudioBufferList *output,
        const AURenderEvent *events,
        AURenderPullInputBlock pull
    ) {
        return [context renderWithFlags:flags time:time frames:frames bus:bus
                                 output:output events:events pull:(__bridge void *)pull];
    };
}
@end

AUAudioUnitStatus sotf_call_pull(
    void *pull,
    AudioUnitRenderActionFlags *flags,
    const AudioTimeStamp *time,
    AUAudioFrameCount frames,
    NSInteger bus,
    AudioBufferList *output
) {
    // Core Audio guarantees the pull block only for this render invocation.
    AURenderPullInputBlock __unsafe_unretained block = (__bridge AURenderPullInputBlock)pull;
    return block(flags, time, frames, bus, output);
}
