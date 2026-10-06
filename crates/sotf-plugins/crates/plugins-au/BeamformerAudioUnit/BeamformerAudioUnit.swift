// BeamformerAudioUnit.swift
// SOTF Beamformer Audio Unit

import AVFoundation

public class BeamformerAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "Beamformer" }
    override public class var pluginSubtype: String { "SOBF" }
    override public class var pluginName: String { "SOTF: Beamformer" }
    override public class var initialInputChannels: AVAudioChannelCount { 2 }
    override public class var initialOutputChannels: AVAudioChannelCount { 1 }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        (2...8).flatMap { [NSNumber(value: $0), NSNumber(value: 1)] }
    }

    override public func pluginConfiguration(inputFormat: AVAudioFormat,
                                             outputFormat: AVAudioFormat) -> String {
        "{\"num_mics\":\(inputFormat.channelCount)}"
    }
}
