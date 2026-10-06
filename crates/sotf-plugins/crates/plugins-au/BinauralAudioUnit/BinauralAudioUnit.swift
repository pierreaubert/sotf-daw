// BinauralAudioUnit.swift
// SOTF Binaural Audio Unit

import AVFoundation

public class BinauralAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "Binaural" }
    override public class var pluginSubtype: String { "SOBn" }
    override public class var pluginName: String { "SOTF: Binaural" }
    override public class var fixedOutputChannels: Int? { 2 }
    override public class var initialInputChannels: AVAudioChannelCount { 2 }
    override public class var initialOutputChannels: AVAudioChannelCount { 2 }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        [1, 2, 3, 5, 6, 8, 10, 12, 14, 16].flatMap { input in
            [NSNumber(value: input), NSNumber(value: 2)]
        }
    }
}
