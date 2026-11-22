#pragma once

#import <AudioToolbox/AUAudioUnitImplementation.h>
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN
@protocol SOTFRenderContext <NSObject>
- (AUAudioUnitStatus)renderWithFlags:(AudioUnitRenderActionFlags *)flags
                               time:(const AudioTimeStamp *)time
                             frames:(AUAudioFrameCount)frames
                                bus:(NSInteger)bus
                             output:(AudioBufferList *)output
                             events:(const AURenderEvent * _Nullable)events
                               pull:(void * _Nullable)pull;
@end

@interface SOTFNativeAudioUnit : AUAudioUnit
- (id<SOTFRenderContext>)makeSOTFRenderContext;
@end

AUAudioUnitStatus sotf_call_pull(
    void *pull,
    AudioUnitRenderActionFlags *flags,
    const AudioTimeStamp *time,
    AUAudioFrameCount frames,
    NSInteger bus,
    AudioBufferList *output
);
NS_ASSUME_NONNULL_END
