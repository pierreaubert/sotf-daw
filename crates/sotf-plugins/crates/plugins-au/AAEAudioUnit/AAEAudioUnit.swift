// AAEAudioUnit.swift
// SOTF AAE Reverb Audio Unit

import AVFoundation

public class AAEAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "AAE" }
    override public class var pluginSubtype: String { "SOAE" }
    override public class var pluginName: String { "SOTF: AAE Reverb" }
    override public class var initialInputChannels: AVAudioChannelCount { 2 }
    override public class var initialOutputChannels: AVAudioChannelCount { 6 }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        [5, 6, 8, 10, 12, 14, 16].flatMap { width in
            [NSNumber(value: 2), NSNumber(value: width)]
        }
    }

    override public func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        guard super.shouldChange(to: format, for: bus) else { return false }
        return bus !== outputBusses[0] || auAaeSpeakerLayout(format) != nil
    }

    override public func pluginConfiguration(inputFormat: AVAudioFormat,
                                             outputFormat: AVAudioFormat) -> String {
        guard let layout = auAaeSpeakerLayout(outputFormat) else { return "{invalid}" }
        return "{\"speaker_config\":\"\(layout)\"}"
    }
}

/// Discrete outputs use this AU mapping: 5→5.0, 6→5.1, 8→5.1.2, 10→5.1.4,
/// 12→7.1.4, 14→9.1.4, and 16→9.1.6. Named tags require matching Rust order.
private func auAaeSpeakerLayout(_ format: AVAudioFormat) -> String? {
    let channels = Int(format.channelCount)
    if let tag = format.channelLayout?.layoutTag {
        let discreteTag = kAudioChannelLayoutTag_DiscreteInOrder | AudioChannelLayoutTag(channels)
        if tag != discreteTag {
            switch tag {
            case kAudioChannelLayoutTag_MPEG_5_0_A: return "5.0"
            case kAudioChannelLayoutTag_MPEG_5_1_A: return "5.1"
            case kAudioChannelLayoutTag_MPEG_7_1_C: return "7.1"
            case kAudioChannelLayoutTag_Atmos_5_1_4: return "5.1.4"
            case kAudioChannelLayoutTag_Atmos_7_1_4: return "7.1.4"
            default: return nil
            }
        }
    }
    switch channels {
    case 5: return "5.0"
    case 6: return "5.1"
    case 8: return "5.1.2"
    case 10: return "5.1.4"
    case 12: return "7.1.4"
    case 14: return "9.1.4"
    case 16: return "9.1.6"
    default: return nil
    }
}
