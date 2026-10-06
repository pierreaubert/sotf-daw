// AmbisonicsAudioUnit.swift
// SOTF Ambisonics Decoder Audio Unit

import AVFoundation

public class AmbisonicsAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "AmbisonicsDecoder" }
    override public class var pluginSubtype: String { "SOAm" }
    override public class var pluginName: String { "SOTF: Ambisonics Decoder" }
    override public class var initialInputChannels: AVAudioChannelCount { 4 }
    override public class var initialOutputChannels: AVAudioChannelCount { 6 }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        [4, 9, 16, 25].flatMap { input in
            [6, 8, 10, 12, 14, 16].flatMap { output in
                [NSNumber(value: input), NSNumber(value: output)]
            }
        }
    }

    override public func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        guard super.shouldChange(to: format, for: bus) else { return false }
        return bus !== outputBusses[0] || auAmbisonicsSpeakerLayout(format) != nil
    }

    override public func pluginConfiguration(inputFormat: AVAudioFormat,
                                             outputFormat: AVAudioFormat) -> String {
        let inputChannels = Int(inputFormat.channelCount)
        let order = Int(sqrt(Double(inputChannels))) - 1
        guard let target = auAmbisonicsSpeakerLayout(outputFormat) else { return "{invalid}" }
        return "{\"order\":\(order),\"target_layout\":\"\(target)\"}"
    }
}

/// Discrete outputs map 6→5.1, 8→7.1, 10→5.1.4, 12→7.1.4, 14→9.1.4, and
/// 16→9.1.6. Named tags are accepted only when their speaker order is exact.
private func auAmbisonicsSpeakerLayout(_ format: AVAudioFormat) -> String? {
    let channels = Int(format.channelCount)
    if let tag = format.channelLayout?.layoutTag {
        let discreteTag = kAudioChannelLayoutTag_DiscreteInOrder | AudioChannelLayoutTag(channels)
        if tag != discreteTag {
            switch tag {
            case kAudioChannelLayoutTag_MPEG_5_1_A: return "5.1"
            case kAudioChannelLayoutTag_MPEG_7_1_C: return "7.1"
            case kAudioChannelLayoutTag_Atmos_5_1_4: return "5.1.4"
            case kAudioChannelLayoutTag_Atmos_7_1_4: return "7.1.4"
            default: return nil
            }
        }
    }
    switch channels {
    case 6: return "5.1"
    case 8: return "7.1"
    case 10: return "5.1.4"
    case 12: return "7.1.4"
    case 14: return "9.1.4"
    case 16: return "9.1.6"
    default: return nil
    }
}
