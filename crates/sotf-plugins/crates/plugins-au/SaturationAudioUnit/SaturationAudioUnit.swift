// SaturationAudioUnit.swift
// SOTF Saturation Audio Unit

import AVFoundation

public class SaturationAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "Saturation" }
    override public class var pluginSubtype: String { "SOSt" }
    override public class var pluginName: String { "SOTF: Saturation" }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        (1...32).flatMap { width in [NSNumber(value: width), NSNumber(value: width)] }
    }
}
