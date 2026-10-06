// CrossoverAudioUnit.swift
// SOTF Crossover Audio Unit

import AVFoundation

public class CrossoverAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "Crossover" }
    override public class var pluginSubtype: String { "SOCx" }
    override public class var pluginName: String { "SOTF: Crossover" }
    override public class var initialInputChannels: AVAudioChannelCount { 2 }
    override public class var initialOutputChannels: AVAudioChannelCount { 4 }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        (1...16).flatMap { [NSNumber(value: $0), NSNumber(value: $0 * 2)] }
    }

    override public func pluginConfiguration(inputFormat: AVAudioFormat,
                                             outputFormat: AVAudioFormat) -> String {
        "{\"output\":\"both\"}"
    }
}
