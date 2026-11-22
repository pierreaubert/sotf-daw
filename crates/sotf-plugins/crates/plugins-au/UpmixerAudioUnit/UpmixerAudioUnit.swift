// UpmixerAudioUnit.swift
// SOTF Upmixer Audio Unit

import AVFoundation

public class UpmixerAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "Upmixer" }
    override public class var pluginSubtype: String { "SOUp" }
    override public class var pluginName: String { "SOTF: Upmixer" }
    override public class var initialInputChannels: AVAudioChannelCount { 2 }
    override public class var initialOutputChannels: AVAudioChannelCount { 6 }
    override public class var refreshParameterTreeAfterInactiveStateRestore: Bool { true }
    override public class func parameterIsPortableAcrossTopologies(_ identifier: String) -> Bool {
        identifier != "speaker_config" && identifier != "input_format"
    }
    override public class func stateValueForReconstruction(_ identifier: String, value: AUValue) -> Any? {
        switch identifier {
        case "bass_mode": return Int(value.rounded())
        case "bass_cutoff_hz", "bass_gain_db", "lfe_playback_gain_db": return Double(value)
        case "combine_input_lfe", "bass_source_front_left", "bass_source_front_right",
             "bass_source_center", "bass_source_side_left", "bass_source_side_right",
             "bass_source_back_left", "bass_source_back_right": return value >= 0.5
        default: return nil
        }
    }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        [2, 5, 6, 8, 10, 12, 14, 16].flatMap { width in
            [NSNumber(value: 2), NSNumber(value: width)]
        }
    }

    override public func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        guard super.shouldChange(to: format, for: bus) else { return false }
        return bus !== outputBusses[0] || auUpmixerSpeakerLayout(format) != nil
    }

    override public func pluginConfiguration(inputFormat: AVAudioFormat,
                                             outputFormat: AVAudioFormat) -> String {
        guard let layout = auUpmixerSpeakerLayout(outputFormat) else { return "{invalid}" }
        return "{\"speaker_config\":\"\(layout)\"}"
    }
}

/// Discrete outputs map 2→2.0, 5→5.0, 6→5.1, 8→5.1.2, 10→5.1.4, 12→7.1.4,
/// 14→9.1.4, and 16→9.1.6. Named tags require matching Rust speaker order.
private func auUpmixerSpeakerLayout(_ format: AVAudioFormat) -> String? {
    let channels = Int(format.channelCount)
    if let tag = format.channelLayout?.layoutTag {
        let discreteTag = kAudioChannelLayoutTag_DiscreteInOrder | AudioChannelLayoutTag(channels)
        if tag != discreteTag {
            switch tag {
            case kAudioChannelLayoutTag_Stereo: return "2.0"
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
    case 2: return "2.0"
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
