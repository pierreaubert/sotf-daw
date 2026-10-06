// DownmixAudioUnit.swift
// SOTF Downmix Audio Unit

import AVFoundation

public class DownmixAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "Downmix" }
    override public class var pluginSubtype: String { "SODm" }
    override public class var pluginName: String { "SOTF: Downmix" }
    override public class var fixedOutputChannels: Int? { 2 }
    override public class var initialInputChannels: AVAudioChannelCount { 6 }
    override public class var supportedChannelCapabilities: [NSNumber]? { [-32, 2] }

    override public func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        guard super.shouldChange(to: format, for: bus) else { return false }
        if bus === inputBusses[0] { return auDownmixFormatSupported(format) }
        return bus === outputBusses[0]
    }

    override public func pluginConfiguration(inputFormat: AVAudioFormat,
                                             outputFormat: AVAudioFormat) -> String {
        let channels = Int(inputFormat.channelCount)
        let layout = auDownmixSpeakerLayout(inputFormat)
        if let layout = layout {
            return "{\"input_channels\":\(channels),\"input_layout\":\"\(layout)\"}"
        }
        guard auDownmixFormatSupported(inputFormat) else { return "{invalid}" }
        // Unsupported named tags and ambiguous discrete widths carry no
        // layout identity; their format negotiation is rejected above.
        return "{\"input_channels\":\(channels)}"
    }
}

/// Maps documented tags; discrete 8, 10, and 12-channel inputs remain ambiguous.
private func auDownmixSpeakerLayout(_ format: AVAudioFormat) -> String? {
    let channels = Int(format.channelCount)
    if let tag = format.channelLayout?.layoutTag {
        let discreteTag = kAudioChannelLayoutTag_DiscreteInOrder | AudioChannelLayoutTag(channels)
        if tag != discreteTag {
            switch tag {
            case kAudioChannelLayoutTag_Stereo: return "2.0"
            case kAudioChannelLayoutTag_MPEG_5_0_A: return "5.0"
            case kAudioChannelLayoutTag_MPEG_5_1_A: return "5.1"
            case kAudioChannelLayoutTag_MPEG_7_1_C: return "7.1"
            case kAudioChannelLayoutTag_Atmos_5_1_2: return "coreaudio_atmos_5.1.2"
            case kAudioChannelLayoutTag_Atmos_7_1_2: return "coreaudio_atmos_7.1.2"
            case kAudioChannelLayoutTag_Atmos_9_1_6: return "coreaudio_atmos_9.1.6"
            case kAudioChannelLayoutTag_Atmos_5_1_4: return "5.1.4"
            case kAudioChannelLayoutTag_Atmos_7_1_4: return "7.1.4"
            default: return nil
            }
        }
    }
    if channels == 8 || channels == 10 { return nil }
    return [1: "1.0", 2: "2.0", 3: "2.1", 5: "5.0", 6: "5.1",
            14: "9.1.4", 16: "9.1.6"][channels]
}

private func auDownmixFormatSupported(_ format: AVAudioFormat) -> Bool {
    let channels = Int(format.channelCount)
    guard let tag = format.channelLayout?.layoutTag else {
        return channels != 8 && channels != 10 && channels != 12
    }
    let discreteTag = kAudioChannelLayoutTag_DiscreteInOrder | AudioChannelLayoutTag(channels)
    if tag == discreteTag {
        return channels != 8 && channels != 10 && channels != 12
    }
    return auDownmixSpeakerLayout(format) != nil
}
