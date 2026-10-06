// MonoToStereoAudioUnit.swift
// SOTF MonoToStereo Audio Unit

import AVFoundation

public class MonoToStereoAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "MonoToStereo" }
    override public class var pluginSubtype: String { "SOM2" }
    override public class var pluginName: String { "SOTF: Mono to Stereo" }
    override public class var fixedInputChannels: Int? { 1 }
    override public class var fixedOutputChannels: Int? { 2 }
    override public class var initialInputChannels: AVAudioChannelCount { 1 }
    override public class var initialOutputChannels: AVAudioChannelCount { 2 }
}
