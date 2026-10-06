// BandMergeAudioUnit.swift
// SOTF Band Merge Audio Unit

import AVFoundation

public class BandMergeAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "BandMerge" }
    override public class var pluginSubtype: String { "SOBM" }
    override public class var pluginName: String { "SOTF: Band Merge" }
    override public class var initialInputChannels: AVAudioChannelCount { 4 }
    override public class var initialOutputChannels: AVAudioChannelCount { 2 }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        var pairs: [NSNumber] = []
        for bands in 2...8 {
            for output in 1...(32 / bands) {
                pairs.append(contentsOf: [NSNumber(value: output * bands), NSNumber(value: output)])
            }
        }
        return pairs
    }

    override public func pluginConfiguration(inputFormat: AVAudioFormat,
                                             outputFormat: AVAudioFormat) -> String {
        let input = Int(inputFormat.channelCount)
        let output = Int(outputFormat.channelCount)
        guard output > 0, input % output == 0 else { return "{}" }
        return "{\"bands\":\(input / output)}"
    }
}
