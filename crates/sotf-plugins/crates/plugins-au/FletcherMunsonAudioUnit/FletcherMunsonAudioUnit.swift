// FletcherMunsonAudioUnit.swift
// SOTF Fletcher-Munson Audio Unit

import AVFoundation

public class FletcherMunsonAudioUnit: GenericRustAudioUnit {
    override public class var pluginType: String { "FletcherMunson" }
    override public class var pluginSubtype: String { "SOFm" }
    override public class var pluginName: String { "SOTF: Fletcher-Munson" }
    override public class var supportedChannelCapabilities: [NSNumber]? {
        (1...32).flatMap { width in [NSNumber(value: width), NSNumber(value: width)] }
    }
}
