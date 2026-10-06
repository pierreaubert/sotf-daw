// BandSplitAudioUnit.swift
// SOTF Band Split Audio Unit

import AVFoundation

public class BandSplitAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "BandSplit" }
    override public class var pluginSubtype: String { "SOBS" }
    override public class var pluginName: String { "SOTF: Band Split" }
    override public class var initialInputChannels: AVAudioChannelCount { 2 }
    override public class var initialOutputChannels: AVAudioChannelCount { 4 }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        var pairs: [NSNumber] = []
        for bands in 2...4 {
            for input in 1...(32 / bands) {
                pairs.append(contentsOf: [NSNumber(value: input), NSNumber(value: input * bands)])
            }
        }
        return pairs
    }

    override public func pluginConfiguration(inputFormat: AVAudioFormat,
                                             outputFormat: AVAudioFormat) -> String {
        let input = Int(inputFormat.channelCount)
        let output = Int(outputFormat.channelCount)
        guard input > 0, output % input == 0 else { return "{}" }
        let bands = output / input
        return "{\"num_bands\":\(bands)}"
    }
}
