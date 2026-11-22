import AVFoundation
import Synchronization
import AudioToolbox
import Darwin
import Foundation

@_silgen_name("sotf_malloc_counter_begin")
private func sotfMallocCounterBegin()
@_silgen_name("sotf_malloc_counter_end")
private func sotfMallocCounterEnd(
    _ allocations: UnsafeMutablePointer<UInt64>,
    _ frees: UnsafeMutablePointer<UInt64>
)
@_silgen_name("sotf_malloc_counter_self_test")
private func sotfMallocCounterSelfTest(
    _ allocations: UnsafeMutablePointer<UInt64>,
    _ frees: UnsafeMutablePointer<UInt64>
)
@_silgen_name("sotf_malloc_counter_dump_callers")
private func sotfMallocCounterDumpCallers()
@_silgen_name("sotf_render_counter_arm")
private func sotfRenderCounterArm()
@_silgen_name("sotf_render_counter_disarm")
private func sotfRenderCounterDisarm(
    _ allocations: UnsafeMutablePointer<UInt64>,
    _ frees: UnsafeMutablePointer<UInt64>
)

private struct DownmixLayoutCase {
    let name: String
    let channels: AVAudioChannelCount
    let tag: AudioChannelLayoutTag
    let labels: [String]
    let configurationID: String?

    init(
        name: String,
        channels: AVAudioChannelCount,
        tag: AudioChannelLayoutTag,
        labels: [String],
        configurationID: String? = nil
    ) {
        self.name = name
        self.channels = channels
        self.tag = tag
        self.labels = labels
        self.configurationID = configurationID
    }
}

private final class MatchingCapabilitiesAudioUnit: GainAudioUnit {
    override class var pluginSubtype: String { "SOmc" }
    override class var supportedChannelCapabilities: [NSNumber]? { [-1, -1] }
}

private final class IndependentCapabilitiesAudioUnit: GainAudioUnit {
    override class var pluginSubtype: String { "SOic" }
    override class var supportedChannelCapabilities: [NSNumber]? { [-1, -2] }
}

private func testAuthoritativeChannelCapabilitySemantics() throws {
    try require(
        MatchingCapabilitiesAudioUnit.channelPairAllowedForTesting(
            inputChannels: 6,
            outputChannels: 6
        ),
        "AUChannelInfo -1/-1 rejected matching widths"
    )
    try require(
        !MatchingCapabilitiesAudioUnit.channelPairAllowedForTesting(
            inputChannels: 6,
            outputChannels: 2
        ),
        "AUChannelInfo -1/-1 accepted nonmatching widths"
    )
    try require(
        IndependentCapabilitiesAudioUnit.channelPairAllowedForTesting(
            inputChannels: 6,
            outputChannels: 2
        ),
        "AUChannelInfo -1/-2 rejected independent widths"
    )
}

private func testAbsoluteEventTimelineConversion() throws {
    let translated = GainAudioUnit.eventTimelineForTesting(
        eventTime: 4_112,
        blockStart: 4_096,
        valid: true,
        maximumOffset: 63
    )
    try require(
        translated.offset == 16 && translated.outputTime == 4_112,
        "valid AU timeline was not translated both directions: \(translated)"
    )
    let relative = GainAudioUnit.eventTimelineForTesting(
        eventTime: 16,
        blockStart: 4_096,
        valid: false,
        maximumOffset: 63
    )
    try require(
        relative.offset == 16 && relative.outputTime == 16,
        "invalid sample-time fallback did not preserve relative events: \(relative)"
    )
}

private func testQueuedControlHasBoundedRenderFairness() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let renderClaims = unit.renderBurstBeforeQueuedControlWinsForTesting()
    try require(renderClaims >= 0, "control fairness harness failed")
    try require(
        renderClaims <= 8,
        "queued control was starved by \(renderClaims) render claims"
    )
}

private struct ConstructionCase {
    let name: String
    let construct: () throws -> GenericRustAudioUnit
}

private struct TopologyCase {
    let name: String
    let inputChannels: AVAudioChannelCount
    let outputChannels: AVAudioChannelCount
    let activeInputChannel: Int?
    let construct: () throws -> GenericRustAudioUnit
}

private final class StressFailureBox: @unchecked Sendable {
    private let lock = NSLock()
    private var message: String?

    func record(_ value: String) {
        lock.lock()
        if message == nil { message = value }
        lock.unlock()
    }

    func take() -> String? {
        lock.lock()
        defer { lock.unlock() }
        return message
    }
}

private let binauralCapabilities: [NSNumber] = [
    1, 2, 2, 2, 3, 2, 5, 2, 6, 2, 8, 2, 10, 2, 12, 2, 14, 2, 16, 2,
]
private let downmixCapabilities: [NSNumber] = [-32, 2]

private enum SmokeFailure: Error, CustomStringConvertible {
    case failure(String)

    var description: String {
        switch self {
        case .failure(let message): return message
        }
    }
}

private final class InvalidAudioUnit: GenericRustAudioUnit {
    override class var pluginType: String { "Gain" }
    override class var pluginSubtype: String { "SOBD" }
    override class var initialInputChannels: AVAudioChannelCount { 0 }
}

private func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    if !condition() {
        throw SmokeFailure.failure(message)
    }
}

private func fourCC(_ value: String) -> FourCharCode {
    value.utf8.reduce(0) { ($0 << 8) | FourCharCode($1) }
}

private func discreteTag(_ channels: AVAudioChannelCount) -> AudioChannelLayoutTag {
    kAudioChannelLayoutTag_DiscreteInOrder | AudioChannelLayoutTag(channels)
}

private func componentDescription(_ subtype: String) -> AudioComponentDescription {
    AudioComponentDescription(
        componentType: kAudioUnitType_Effect,
        componentSubType: fourCC(subtype),
        componentManufacturer: fourCC("SOTF"),
        componentFlags: 0,
        componentFlagsMask: 0
    )
}

private func format(
    sampleRate: Double,
    channels: AVAudioChannelCount,
    tag: AudioChannelLayoutTag
) throws -> AVAudioFormat {
    guard let layout = AVAudioChannelLayout(layoutTag: tag) else {
        throw SmokeFailure.failure("cannot construct Core Audio layout tag \(tag)")
    }
    guard layout.channelCount == channels else {
        throw SmokeFailure.failure(
            "layout tag \(tag) exposes \(layout.channelCount) channels, expected \(channels)"
        )
    }
    let format = AVAudioFormat(
        standardFormatWithSampleRate: sampleRate,
        channelLayout: layout
    )
    return format
}

private func stereoFormat(sampleRate: Double) throws -> AVAudioFormat {
    guard let format = AVAudioFormat(
        standardFormatWithSampleRate: sampleRate,
        channels: 2
    ) else {
        throw SmokeFailure.failure("cannot construct stereo AVAudioFormat")
    }
    return format
}

private func plainFormat(
    sampleRate: Double,
    channels: AVAudioChannelCount
) throws -> AVAudioFormat {
    if channels > 2 {
        return try format(
            sampleRate: sampleRate,
            channels: channels,
            tag: discreteTag(channels)
        )
    }
    guard let format = AVAudioFormat(
        standardFormatWithSampleRate: sampleRate,
        channels: channels
    ) else {
        throw SmokeFailure.failure("cannot construct \(channels)-channel AVAudioFormat")
    }
    return format
}

private func render(
    _ unit: AUAudioUnit,
    inputFormat: AVAudioFormat,
    outputFormat: AVAudioFormat,
    frames: AVAudioFrameCount,
    activeInputChannel: Int? = nil
) throws -> (AVAudioPCMBuffer, AVAudioPCMBuffer) {
    try unit.inputBusses[0].setFormat(inputFormat)
    try unit.outputBusses[0].setFormat(outputFormat)
    unit.maximumFramesToRender = frames
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }

    guard let input = AVAudioPCMBuffer(pcmFormat: inputFormat, frameCapacity: frames),
          let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: frames) else {
        throw SmokeFailure.failure("cannot allocate native render buffers")
    }
    input.frameLength = frames
    output.frameLength = frames

    guard let channels = input.floatChannelData else {
        throw SmokeFailure.failure("native input format is not Float32 planar")
    }
    for channel in 0..<Int(inputFormat.channelCount) {
        for frame in 0..<Int(frames) {
            let phase = Float(frame % 97) / 97.0
            channels[channel][frame] = activeInputChannel == nil || activeInputChannel == channel
                ? 0.02 + 0.1 * phase
                : 0.0
        }
    }
    if let outputChannels = output.floatChannelData {
        for channel in 0..<Int(outputFormat.channelCount) {
            for frame in 0..<Int(frames) {
                outputChannels[channel][frame] = .nan
            }
        }
    }

    let pullInput: AURenderPullInputBlock = {
        _, _, requestedFrames, _, destination in
        let sourceBuffers = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
        let destinationBuffers = UnsafeMutableAudioBufferListPointer(destination)
        guard requestedFrames <= input.frameLength,
              sourceBuffers.count == destinationBuffers.count else {
            return kAudioUnitErr_InvalidPropertyValue
        }
        for index in 0..<sourceBuffers.count {
            guard let source = sourceBuffers[index].mData,
                  let target = destinationBuffers[index].mData else {
                return kAudioUnitErr_NoConnection
            }
            let byteCount = Int(sourceBuffers[index].mDataByteSize)
            memcpy(target, source, byteCount)
            destinationBuffers[index].mDataByteSize = UInt32(byteCount)
        }
        return noErr
    }

    var flags = AudioUnitRenderActionFlags(rawValue: 0)
    var timestamp = AudioTimeStamp()
    timestamp.mSampleTime = 0
    timestamp.mFlags = .sampleTimeValid
    for blockIndex in 0..<3 {
        if let outputChannels = output.floatChannelData {
            for channel in 0..<Int(outputFormat.channelCount) {
                for frame in 0..<Int(frames) {
                    outputChannels[channel][frame] = .nan
                }
            }
        }
        timestamp.mSampleTime = Float64(blockIndex) * Float64(frames)
        let status = unit.internalRenderBlock(
            &flags,
            &timestamp,
            frames,
            0,
            output.mutableAudioBufferList,
            nil,
            pullInput
        )
        try require(status == noErr, "native render returned OSStatus \(status)")
    }
    return (input, output)
}

private func assertFiniteOverwrittenAndAudible(
    _ output: AVAudioPCMBuffer,
    caseName: String,
    allowedSilentChannels: Set<Int> = []
) throws {
    guard let channels = output.floatChannelData else {
        throw SmokeFailure.failure("\(caseName): native output is not Float32 planar")
    }
    for channel in 0..<Int(output.format.channelCount) {
        var energy = 0.0
        for frame in 0..<Int(output.frameLength) {
            let sample = channels[channel][frame]
            try require(sample.isFinite, "\(caseName): output channel \(channel) was not overwritten")
            energy += Double(sample * sample)
        }
        if !allowedSilentChannels.contains(channel) {
            try require(
                energy > 1.0e-8,
                "\(caseName): output channel \(channel) produced only silence"
            )
        }
    }
}

private func stereoEnergy(_ output: AVAudioPCMBuffer) throws -> (Double, Double) {
    guard let channels = output.floatChannelData else {
        throw SmokeFailure.failure("native output is not Float32 planar")
    }
    var left = 0.0
    var right = 0.0
    for frame in 0..<Int(output.frameLength) {
        let l = channels[0][frame]
        let r = channels[1][frame]
        try require(l.isFinite && r.isFinite, "native output was not completely overwritten")
        left += Double(l * l)
        right += Double(r * r)
    }
    return (left, right)
}

private func assertFiniteAudibleStereo(
    _ output: AVAudioPCMBuffer,
    caseName: String
) throws {
    guard let channels = output.floatChannelData else {
        throw SmokeFailure.failure("\(caseName): native output is not Float32 planar")
    }
    var energy = 0.0
    for channel in 0..<2 {
        for frame in 0..<Int(output.frameLength) {
            let sample = channels[channel][frame]
            try require(sample.isFinite, "\(caseName): non-finite output sample")
            energy += Double(sample * sample)
        }
    }
    try require(energy > 1.0e-8, "\(caseName): native render produced only silence")
}

private func testEveryPublishedWrapperConstructs() throws {
    let cases = [
        ConstructionCase(name: "AAE") { try AAEAudioUnit(componentDescription: componentDescription("SOAE")) },
        ConstructionCase(name: "ABCompare") { try ABCompareAudioUnit(componentDescription: componentDescription("SOAb")) },
        ConstructionCase(name: "AEC") { try AECAudioUnit(componentDescription: componentDescription("SOEc")) },
        ConstructionCase(name: "Ambisonics") { try AmbisonicsAudioUnit(componentDescription: componentDescription("SOAm")) },
        ConstructionCase(name: "BandMerge") { try BandMergeAudioUnit(componentDescription: componentDescription("SOBM")) },
        ConstructionCase(name: "BandSplit") { try BandSplitAudioUnit(componentDescription: componentDescription("SOBS")) },
        ConstructionCase(name: "Beamformer") { try BeamformerAudioUnit(componentDescription: componentDescription("SOBF")) },
        ConstructionCase(name: "Binaural") { try BinauralAudioUnit(componentDescription: componentDescription("SOBn")) },
        ConstructionCase(name: "ChannelMuteSolo") { try ChannelMuteSoloAudioUnit(componentDescription: componentDescription("SOCs")) },
        ConstructionCase(name: "Compressor") { try CompressorAudioUnit(componentDescription: componentDescription("SOCP")) },
        ConstructionCase(name: "Convolution") { try ConvolutionAudioUnit(componentDescription: componentDescription("SOCv")) },
        ConstructionCase(name: "Crossfeed") { try CrossfeedAudioUnit(componentDescription: componentDescription("SOCf")) },
        ConstructionCase(name: "Crossover") { try CrossoverAudioUnit(componentDescription: componentDescription("SOCx")) },
        ConstructionCase(name: "DeEsser") { try DeEsserAudioUnit(componentDescription: componentDescription("SODs")) },
        ConstructionCase(name: "Declick") { try DeclickAudioUnit(componentDescription: componentDescription("SODc")) },
        ConstructionCase(name: "Delay") { try DelayAudioUnit(componentDescription: componentDescription("SODY")) },
        ConstructionCase(name: "Denoiser") { try DenoiserAudioUnit(componentDescription: componentDescription("SODn")) },
        ConstructionCase(name: "Dither") { try DitherAudioUnit(componentDescription: componentDescription("SODt")) },
        ConstructionCase(name: "Downmix") { try DownmixAudioUnit(componentDescription: componentDescription("SODm")) },
        ConstructionCase(name: "DynamicEQ") { try DynamicEQAudioUnit(componentDescription: componentDescription("SODq")) },
        ConstructionCase(name: "EQ") { try EQAudioUnit(componentDescription: componentDescription("SOEQ")) },
        ConstructionCase(name: "Expander") { try ExpanderAudioUnit(componentDescription: componentDescription("SOEx")) },
        ConstructionCase(name: "FletcherMunson") { try FletcherMunsonAudioUnit(componentDescription: componentDescription("SOFm")) },
        ConstructionCase(name: "Gain") { try GainAudioUnit(componentDescription: componentDescription("SOGN")) },
        ConstructionCase(name: "Gate") { try GateAudioUnit(componentDescription: componentDescription("SOGT")) },
        ConstructionCase(name: "HissReducer") { try HissReducerAudioUnit(componentDescription: componentDescription("SOHr")) },
        ConstructionCase(name: "Limiter") { try LimiterAudioUnit(componentDescription: componentDescription("SOLM")) },
        ConstructionCase(name: "LinearPhaseEQ") { try LinearPhaseEQAudioUnit(componentDescription: componentDescription("SOLP")) },
        ConstructionCase(name: "LoudnessCompensation") { try LoudnessCompensationAudioUnit(componentDescription: componentDescription("SOLc")) },
        ConstructionCase(name: "LoudnessMonitor") { try LoudnessMonitorAudioUnit(componentDescription: componentDescription("SOLu")) },
        ConstructionCase(name: "Matrix") { try MatrixAudioUnit(componentDescription: componentDescription("SOMx")) },
        ConstructionCase(name: "MonoToStereo") { try MonoToStereoAudioUnit(componentDescription: componentDescription("SOM2")) },
        ConstructionCase(name: "MultibandCompressor") { try MultibandCompressorAudioUnit(componentDescription: componentDescription("SOMc")) },
        ConstructionCase(name: "MultibandExpander") { try MultibandExpanderAudioUnit(componentDescription: componentDescription("SOMe")) },
        ConstructionCase(name: "PND") { try PNDAudioUnit(componentDescription: componentDescription("SOPn")) },
        ConstructionCase(name: "Saturation") { try SaturationAudioUnit(componentDescription: componentDescription("SOSt")) },
        ConstructionCase(name: "SpectralCompressor") { try SpectralCompressorAudioUnit(componentDescription: componentDescription("SOSC")) },
        ConstructionCase(name: "SpectrumAnalyzer") { try SpectrumAnalyzerAudioUnit(componentDescription: componentDescription("SOSa")) },
        ConstructionCase(name: "SpeechDenoiser") { try SpeechDenoiserAudioUnit(componentDescription: componentDescription("SOSd")) },
        ConstructionCase(name: "StereoImager") { try StereoImagerAudioUnit(componentDescription: componentDescription("SOSi")) },
        ConstructionCase(name: "TransientShaper") { try TransientShaperAudioUnit(componentDescription: componentDescription("SOTs")) },
        ConstructionCase(name: "Upmixer") { try UpmixerAudioUnit(componentDescription: componentDescription("SOUp")) },
        ConstructionCase(name: "XTC") { try XTCAudioUnit(componentDescription: componentDescription("SOXt")) },
    ]

    for wrapperCase in cases {
        let unit: GenericRustAudioUnit
        do {
            unit = try wrapperCase.construct()
        } catch {
            throw SmokeFailure.failure("\(wrapperCase.name): construction failed: \(error)")
        }
        try require(unit.hasRustPlugin, "\(wrapperCase.name): provisional Rust plugin missing")
        try require(!unit.supportsMPE, "\(wrapperCase.name): aufx falsely advertises MPE")
        try require(
            unit.midiOutputNames.isEmpty,
            "\(wrapperCase.name): aufx falsely advertises MIDI output"
        )
        for parameter in unit.parameterTree?.allParameters ?? [] {
            guard let steps = unit.parameterStepsForTesting(address: parameter.address) else {
                throw SmokeFailure.failure(
                    "\(wrapperCase.name): missing metadata for \(parameter.identifier)"
                )
            }
            guard let realtime = unit.parameterIsRealtimeForTesting(address: parameter.address) else {
                throw SmokeFailure.failure(
                    "\(wrapperCase.name): missing update mode for \(parameter.identifier)"
                )
            }
            guard let floatingPoint = unit.parameterIsFloatingPointForTesting(address: parameter.address) else {
                throw SmokeFailure.failure(
                    "\(wrapperCase.name): missing type for \(parameter.identifier)"
                )
            }
            let preciseRamp = wrapperCase.name == "Upmixer" && parameter.identifier == "height_gain"
            try require(
                parameter.flags.contains(.flag_CanRamp) == (floatingPoint && realtime && (steps == 0 || preciseRamp)),
                "\(wrapperCase.name).\(parameter.identifier): CanRamp disagrees with steps=\(steps), floatingPoint=\(floatingPoint), realtime=\(realtime)"
            )
        }
        try require(
            unit.inputBusses[0].format.channelCount == type(of: unit).initialInputChannels,
            "\(wrapperCase.name): provisional input width is not truthful"
        )
        try require(
            unit.outputBusses[0].format.channelCount == type(of: unit).initialOutputChannels,
            "\(wrapperCase.name): provisional output width is not truthful"
        )
    }

    guard let projectPath = ProcessInfo.processInfo.environment["SOTF_AU_PROJECT_YML"] else {
        throw SmokeFailure.failure("SOTF_AU_PROJECT_YML was not supplied by the native test recipe")
    }
    let project = try String(contentsOfFile: projectPath, encoding: .utf8)
    let projectLines = project.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    let publishedTargets = Set(projectLines.compactMap { line -> String? in
        guard line.hasPrefix("  "), !line.hasPrefix("    "), line.hasSuffix("AudioUnit:") else {
            return nil
        }
        return String(line.dropFirst(2).dropLast())
    })
    let testedTargets = Set(cases.map { "\($0.name)AudioUnit" })
    var embeddedTargets = Set<String>()
    var inMainApplication = false
    for (index, line) in projectLines.enumerated() {
        if line == "  SOTFAudioUnits:" {
            inMainApplication = true
            continue
        }
        if inMainApplication, line.hasPrefix("  "), !line.hasPrefix("    ") {
            break
        }
        guard inMainApplication,
              line.hasPrefix("      - target: ") else { continue }
        let target = String(line.dropFirst("      - target: ".count))
        let embedded = projectLines[(index + 1)...].first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
        if embedded?.trimmingCharacters(in: .whitespaces) == "embed: true" {
            embeddedTargets.insert(target)
        }
    }
    let projectDirectory = URL(fileURLWithPath: projectPath).deletingLastPathComponent()
    let sourceTargets = Set(try FileManager.default.contentsOfDirectory(
        at: projectDirectory,
        includingPropertiesForKeys: [.isDirectoryKey]
    ).compactMap { url -> String? in
        let name = url.lastPathComponent
        guard name.hasSuffix("AudioUnit"), name != "EQiOSAudioUnit" else { return nil }
        let source = url.appendingPathComponent("\(name).swift")
        return FileManager.default.fileExists(atPath: source.path) ? name : nil
    })
    try require(
        testedTargets == publishedTargets,
        "native wrapper inventory mismatch; untested=\(publishedTargets.subtracting(testedTargets).sorted()), unpublished=\(testedTargets.subtracting(publishedTargets).sorted())"
    )
    try require(
        embeddedTargets == publishedTargets,
        "embedded extension inventory mismatch; not-embedded=\(publishedTargets.subtracting(embeddedTargets).sorted()), embedded-without-target=\(embeddedTargets.subtracting(publishedTargets).sorted())"
    )
    try require(
        sourceTargets == publishedTargets,
        "wrapper source inventory mismatch; source-without-target=\(sourceTargets.subtracting(publishedTargets).sorted()), target-without-source=\(publishedTargets.subtracting(sourceTargets).sorted())"
    )
}

private func testDefaultChannelChangingTopologies() throws {
    let cases = [
        TopologyCase(name: "AAE", inputChannels: 2, outputChannels: 6, activeInputChannel: nil) {
            try AAEAudioUnit(componentDescription: componentDescription("SOAE"))
        },
        TopologyCase(name: "AEC", inputChannels: 2, outputChannels: 1, activeInputChannel: 0) {
            try AECAudioUnit(componentDescription: componentDescription("SOEc"))
        },
        TopologyCase(name: "Ambisonics", inputChannels: 4, outputChannels: 6, activeInputChannel: 0) {
            try AmbisonicsAudioUnit(componentDescription: componentDescription("SOAm"))
        },
        TopologyCase(name: "BandSplit", inputChannels: 2, outputChannels: 4, activeInputChannel: nil) {
            try BandSplitAudioUnit(componentDescription: componentDescription("SOBS"))
        },
        TopologyCase(name: "BandMerge", inputChannels: 4, outputChannels: 2, activeInputChannel: nil) {
            try BandMergeAudioUnit(componentDescription: componentDescription("SOBM"))
        },
        TopologyCase(name: "Beamformer", inputChannels: 2, outputChannels: 1, activeInputChannel: nil) {
            try BeamformerAudioUnit(componentDescription: componentDescription("SOBF"))
        },
        TopologyCase(name: "Binaural", inputChannels: 2, outputChannels: 2, activeInputChannel: nil) {
            try BinauralAudioUnit(componentDescription: componentDescription("SOBn"))
        },
        TopologyCase(name: "Crossover", inputChannels: 2, outputChannels: 4, activeInputChannel: nil) {
            try CrossoverAudioUnit(componentDescription: componentDescription("SOCx"))
        },
        TopologyCase(name: "Downmix", inputChannels: 6, outputChannels: 2, activeInputChannel: nil) {
            try DownmixAudioUnit(componentDescription: componentDescription("SODm"))
        },
        TopologyCase(name: "MonoToStereo", inputChannels: 1, outputChannels: 2, activeInputChannel: nil) {
            try MonoToStereoAudioUnit(componentDescription: componentDescription("SOM2"))
        },
        TopologyCase(name: "Upmixer", inputChannels: 2, outputChannels: 6, activeInputChannel: nil) {
            try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
        },
    ]
    let sampleRate = 48_000.0

    for topologyCase in cases {
        let inputFormat = topologyCase.name == "Downmix"
            ? try format(
                sampleRate: sampleRate,
                channels: 6,
                tag: kAudioChannelLayoutTag_MPEG_5_1_A
            )
            : try plainFormat(sampleRate: sampleRate, channels: topologyCase.inputChannels)
        let outputFormat = try plainFormat(sampleRate: sampleRate, channels: topologyCase.outputChannels)
        let unit: GenericRustAudioUnit
        do {
            unit = try topologyCase.construct()
        } catch {
            throw SmokeFailure.failure("\(topologyCase.name): construction failed: \(error)")
        }
        let expectedCapabilities = type(of: unit).supportedChannelCapabilities ?? [
            NSNumber(value: Int(topologyCase.inputChannels)),
            NSNumber(value: Int(topologyCase.outputChannels)),
        ]
        try require(
            unit.channelCapabilities == expectedCapabilities,
            "\(topologyCase.name): advertised channel capabilities are not truthful"
        )
        let (_, output) = try render(
            unit,
            inputFormat: inputFormat,
            outputFormat: outputFormat,
            frames: 4096,
            activeInputChannel: topologyCase.activeInputChannel
        )
        try require(unit.hasRustPlugin, "\(topologyCase.name): negotiated Rust plugin missing")
        try assertFiniteOverwrittenAndAudible(
            output,
            caseName: topologyCase.name,
            allowedSilentChannels: topologyCase.name == "Ambisonics" ? [3] : []
        )
    }
}

private func testFixedTopologyRejectionAndRecovery() throws {
    let sampleRate = 48_000.0
    let cases = [
        TopologyCase(name: "AAE", inputChannels: 2, outputChannels: 6, activeInputChannel: nil) {
            try AAEAudioUnit(componentDescription: componentDescription("SOAE"))
        },
        TopologyCase(name: "AEC", inputChannels: 2, outputChannels: 1, activeInputChannel: 0) {
            try AECAudioUnit(componentDescription: componentDescription("SOEc"))
        },
        TopologyCase(name: "Ambisonics", inputChannels: 4, outputChannels: 6, activeInputChannel: 0) {
            try AmbisonicsAudioUnit(componentDescription: componentDescription("SOAm"))
        },
        TopologyCase(name: "BandSplit", inputChannels: 2, outputChannels: 4, activeInputChannel: nil) {
            try BandSplitAudioUnit(componentDescription: componentDescription("SOBS"))
        },
        TopologyCase(name: "BandMerge", inputChannels: 4, outputChannels: 2, activeInputChannel: nil) {
            try BandMergeAudioUnit(componentDescription: componentDescription("SOBM"))
        },
        TopologyCase(name: "Beamformer", inputChannels: 2, outputChannels: 1, activeInputChannel: nil) {
            try BeamformerAudioUnit(componentDescription: componentDescription("SOBF"))
        },
        TopologyCase(name: "Crossover", inputChannels: 2, outputChannels: 4, activeInputChannel: nil) {
            try CrossoverAudioUnit(componentDescription: componentDescription("SOCx"))
        },
        TopologyCase(name: "MonoToStereo", inputChannels: 1, outputChannels: 2, activeInputChannel: nil) {
            try MonoToStereoAudioUnit(componentDescription: componentDescription("SOM2"))
        },
        TopologyCase(name: "Upmixer", inputChannels: 2, outputChannels: 6, activeInputChannel: nil) {
            try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
        },
        TopologyCase(name: "Crossfeed", inputChannels: 2, outputChannels: 2, activeInputChannel: nil) {
            try CrossfeedAudioUnit(componentDescription: componentDescription("SOCf"))
        },
        TopologyCase(name: "StereoImager", inputChannels: 2, outputChannels: 2, activeInputChannel: nil) {
            try StereoImagerAudioUnit(componentDescription: componentDescription("SOSi"))
        },
        TopologyCase(name: "XTC", inputChannels: 2, outputChannels: 2, activeInputChannel: nil) {
            try XTCAudioUnit(componentDescription: componentDescription("SOXt"))
        },
    ]

    for topologyCase in cases {
        let unit = try topologyCase.construct()
        let wrongInputChannels: AVAudioChannelCount
        switch topologyCase.name {
        case "BandSplit", "Crossover":
            // Both wrappers accept one input channel; 17 exceeds their pair tables.
            wrongInputChannels = 17
        default:
            wrongInputChannels = topologyCase.inputChannels == 1 ? 2 : 1
        }
        do {
            try unit.inputBusses[0].setFormat(
                try plainFormat(sampleRate: sampleRate, channels: wrongInputChannels)
            )
            try unit.outputBusses[0].setFormat(
                try plainFormat(sampleRate: sampleRate, channels: topologyCase.outputChannels)
            )
            try unit.allocateRenderResources()
            unit.deallocateRenderResources()
            throw SmokeFailure.failure("\(topologyCase.name): unsupported input width was accepted")
        } catch let error as NSError {
            try require(
                error.code == Int(kAudioUnitErr_FormatNotSupported),
                "\(topologyCase.name): wrong input width returned \(error)"
            )
        }

        let (_, recoveredOutput) = try render(
            unit,
            inputFormat: try plainFormat(
                sampleRate: sampleRate,
                channels: topologyCase.inputChannels
            ),
            outputFormat: try plainFormat(
                sampleRate: sampleRate,
                channels: topologyCase.outputChannels
            ),
            frames: 4096,
            activeInputChannel: topologyCase.activeInputChannel
        )
        try assertFiniteOverwrittenAndAudible(
            recoveredOutput,
            caseName: "\(topologyCase.name) recovery",
            allowedSilentChannels: topologyCase.name == "Ambisonics" ? [3] : []
        )

        let wrongOutputChannels: AVAudioChannelCount
        switch topologyCase.name {
        case "BandMerge":
            // Four inputs can validly merge to one output; three is not an integer ratio.
            wrongOutputChannels = 3
        default:
            wrongOutputChannels = topologyCase.outputChannels == 1 ? 2 : 1
        }
        do {
            try unit.inputBusses[0].setFormat(
                try plainFormat(sampleRate: sampleRate, channels: topologyCase.inputChannels)
            )
            try unit.outputBusses[0].setFormat(
                try plainFormat(sampleRate: sampleRate, channels: wrongOutputChannels)
            )
            try unit.allocateRenderResources()
            unit.deallocateRenderResources()
            throw SmokeFailure.failure("\(topologyCase.name): unsupported output width was accepted")
        } catch let error as NSError {
            try require(
                error.code == Int(kAudioUnitErr_FormatNotSupported),
                "\(topologyCase.name): wrong output width returned \(error)"
            )
        }

        let (_, outputRecovery) = try render(
            unit,
            inputFormat: try plainFormat(
                sampleRate: sampleRate,
                channels: topologyCase.inputChannels
            ),
            outputFormat: try plainFormat(
                sampleRate: sampleRate,
                channels: topologyCase.outputChannels
            ),
            frames: 4096,
            activeInputChannel: topologyCase.activeInputChannel
        )
        try assertFiniteOverwrittenAndAudible(
            outputRecovery,
            caseName: "\(topologyCase.name) output-width recovery",
            allowedSilentChannels: topologyCase.name == "Ambisonics" ? [3] : []
        )
    }
}

private func testVariableToStereoCapabilityRejectionAndRecovery() throws {
    let sampleRate = 48_000.0
    let stereo = try stereoFormat(sampleRate: sampleRate)
    let cases: [(String, [NSNumber], [AVAudioChannelCount], () throws -> GenericRustAudioUnit)] = [
        ("Binaural", binauralCapabilities, [4, 7], {
            try BinauralAudioUnit(componentDescription: componentDescription("SOBn"))
        }),
        ("Downmix", downmixCapabilities, [33], {
            try DownmixAudioUnit(componentDescription: componentDescription("SODm"))
        }),
    ]
    for (name, capabilities, unsupportedWidths, construct) in cases {
        let unit = try construct()
        try require(
            unit.channelCapabilities == capabilities,
            "\(name): exact supported-width capability table drifted"
        )
        for unsupported in unsupportedWidths {
            do {
                try unit.inputBusses[0].setFormat(
                    try plainFormat(sampleRate: sampleRate, channels: unsupported)
                )
                try unit.outputBusses[0].setFormat(stereo)
                try unit.allocateRenderResources()
                unit.deallocateRenderResources()
                throw SmokeFailure.failure("\(name): unsupported \(unsupported)→2 format was accepted")
            } catch let error as NSError {
                try require(
                    error.code == Int(kAudioUnitErr_FailedInitialization)
                        || error.code == Int(kAudioUnitErr_FormatNotSupported),
                    "\(name): unsupported width returned \(error)"
                )
            }
            let (_, recovered) = try render(
                unit,
                inputFormat: stereo,
                outputFormat: stereo,
                frames: 4096
            )
            try assertFiniteOverwrittenAndAudible(
                recovered,
                caseName: "\(name) recovery from \(unsupported)ch"
            )
        }
        if name == "Binaural" {
            do {
                try unit.outputBusses[0].setFormat(
                    try plainFormat(sampleRate: sampleRate, channels: 1)
                )
                throw SmokeFailure.failure("Binaural unsupported mono output was accepted")
            } catch let error as NSError {
                try require(
                    error.code == Int(kAudioUnitErr_FormatNotSupported),
                    "Binaural wrong output width returned \(error)"
                )
            }
            let (_, recovered) = try render(
                unit,
                inputFormat: stereo,
                outputFormat: stereo,
                frames: 4096
            )
            try assertFiniteOverwrittenAndAudible(
                recovered,
                caseName: "Binaural wrong-output recovery"
            )
        }
    }
}

private func testUpmixerOutputLayoutRecreation() throws {
    let cases: [(String, AVAudioChannelCount, AudioChannelLayoutTag, AUValue)] = [
        ("5.1.4", 10, kAudioChannelLayoutTag_Atmos_5_1_4, 5),
        ("7.1", 8, kAudioChannelLayoutTag_MPEG_7_1_C, 3),
        ("5.1.2", 8, discreteTag(8), 4),
        ("7.1.4", 12, kAudioChannelLayoutTag_Atmos_7_1_4, 7),
        ("5.0", 5, kAudioChannelLayoutTag_MPEG_5_0_A, 1),
        ("5.1", 6, kAudioChannelLayoutTag_MPEG_5_1_A, 2),
        ("9.1.4", 14, discreteTag(14), 8),
        ("9.1.6", 16, discreteTag(16), 9),
        ("2.0", 2, kAudioChannelLayoutTag_Stereo, 0),
    ]
    let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
    let input = try stereoFormat(sampleRate: 48_000)
    for (name, channels, tag, expectedChoice) in cases {
        let output = try format(sampleRate: 48_000, channels: channels, tag: tag)
        let configuration = unit.pluginConfiguration(inputFormat: input, outputFormat: output)
        let json = try JSONSerialization.jsonObject(with: Data(configuration.utf8)) as? [String: Any]
        try require(json?["speaker_config"] as? String == name,
                    "\(name): incorrect construction identity: \(configuration)")
        let (_, rendered) = try render(unit, inputFormat: input, outputFormat: output, frames: 4096)
        try assertFiniteOverwrittenAndAudible(
            rendered,
            caseName: "Upmixer \(name)",
            allowedSilentChannels: Set(2..<Int(channels))
        )
        let actualChoice = unit.parameterValueForTesting(identifier: "speaker_config") ?? .nan
        try require(abs(actualChoice - expectedChoice) < 1.0e-4,
                    "\(name): format recreation selected \(actualChoice), expected \(expectedChoice)")
    }
}

private func testUpmixerReinitializingStateRestore() throws {
    let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
    guard let initialCutoff = unit.parameterTree?.allParameters.first(where: {
        $0.identifier == "bass_cutoff_hz"
    }) else {
        throw SmokeFailure.failure("Upmixer is missing its crossover parameter")
    }
    _ = initialCutoff.value // Prime Core Audio's cached parameter value.
    let observedTreeChanges = Atomic<Int>(0)
    let observedCutoffBits = Atomic<UInt32>(0)
    let observation = unit.observe(\.parameterTree, options: [.new]) { changed, _ in
        let value = changed.parameterTree?.allParameters.first(where: {
            $0.identifier == "bass_cutoff_hz"
        })?.value ?? .nan
        observedCutoffBits.store(value.bitPattern, ordering: .releasing)
        observedTreeChanges.wrappingAdd(1, ordering: .relaxed)
    }
    defer { observation.invalidate() }
    for wanted: Float in [20, 21.5929, 80, 149.333, 180] {
        guard var state = unit.fullState else {
            throw SmokeFailure.failure("Upmixer could not save state")
        }
        // The host dictionary overrides the plugin blob. Restoring this value
        // needs a fresh DSP instance, not the realtime parameter setter.
        state["bass_cutoff_hz"] = NSNumber(value: wanted)
        unit.fullState = state
        let restored = (unit.fullState?["bass_cutoff_hz"] as? NSNumber)?.floatValue ?? .nan
        try require(abs(restored - wanted) < 0.001,
                    "Upmixer crossover restore wanted \(wanted), got \(restored)")
        let displayed = unit.parameterTree?.allParameters.first(where: {
            $0.identifier == "bass_cutoff_hz"
        })?.value ?? .nan
        try require(abs(displayed - wanted) < 0.001,
                    "Upmixer host parameter wanted \(wanted), got \(displayed)")
        let observed = Float(bitPattern: observedCutoffBits.load(ordering: .acquiring))
        try require(observedTreeChanges.load(ordering: .relaxed) > 0
                        && abs(observed - wanted) < 0.001,
                    "Upmixer did not notify the host of the refreshed parameter tree")
    }
    guard var state = unit.fullState else {
        throw SmokeFailure.failure("Upmixer could not save state after crossover sweep")
    }
    state["bass_gain_db"] = NSNumber(value: -8.25)
    state["bass_source_center"] = NSNumber(value: false)
    unit.fullState = state
    let restored = unit.fullState
    let restoredGain = (restored?["bass_gain_db"] as? NSNumber)?.floatValue ?? .nan
    try require(abs(restoredGain + 8.25) < 0.001,
                "Upmixer bass gain override was not restored: \(restoredGain)")
    try require((restored?["bass_source_center"] as? NSNumber)?.boolValue == false,
                "Upmixer bass source override was not restored")
    state["bass_cutoff_hz"] = NSNumber(value: 181)
    unit.fullState = state
    let afterInvalid = (unit.fullState?["bass_cutoff_hz"] as? NSNumber)?.floatValue ?? .nan
    try require(abs(afterInvalid - 180) < 0.001,
                "invalid Upmixer crossover changed the live state")
}

private func testUpmixerStructuralParameterMetadata() throws {
    let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
    let structural = [
        "bass_mode", "bass_cutoff_hz", "bass_gain_db", "lfe_playback_gain_db",
        "combine_input_lfe", "bass_source_front_left", "bass_source_front_right",
        "bass_source_center", "bass_source_side_left", "bass_source_side_right",
        "bass_source_back_left", "bass_source_back_right", "input_format",
        "speaker_config", "low_latency", "enable_ml_detection", "frequency_resolution",
        "bypass_all_processing", "lfe_cutoff_hz", "bandpass_hz", "height_hf_cap_hz",
    ]
    func checkMetadata(_ stage: String) throws {
        let parameters = unit.parameterTree?.allParameters ?? []
        for identifier in structural {
            guard let parameter = parameters.first(where: { $0.identifier == identifier }) else {
                throw SmokeFailure.failure("\(stage): missing structural parameter \(identifier)")
            }
            try require(parameter.flags.contains(.flag_NonRealTime),
                        "\(stage): \(identifier) must be marked non-realtime")
            try require(!parameter.flags.contains(.flag_CanRamp),
                        "\(stage): \(identifier) must not advertise ramps")
            try require(parameter.flags.contains(.flag_IsReadable) && parameter.flags.contains(.flag_IsWritable),
                        "\(stage): \(identifier) must remain readable and writable")
        }
        guard let height = parameters.first(where: { $0.identifier == "height_gain" }) else {
            throw SmokeFailure.failure("\(stage): missing height gain")
        }
        try require(height.flags.contains(.flag_CanRamp) && !height.flags.contains(.flag_NonRealTime),
                    "\(stage): precise height gain must be realtime-rampable")
        guard let bypass = parameters.first(where: { $0.identifier == "bypass_heights" }) else {
            throw SmokeFailure.failure("\(stage): missing height bypass")
        }
        try require(!bypass.flags.contains(.flag_CanRamp) && !bypass.flags.contains(.flag_NonRealTime),
                    "\(stage): height bypass must be realtime and discrete")
    }
    try checkMetadata("initial")
    guard var state = unit.fullState else { throw SmokeFailure.failure("Upmixer has no state") }
    state["bass_cutoff_hz"] = NSNumber(value: 90)
    unit.fullState = state
    let restored = (unit.fullState?["bass_cutoff_hz"] as? NSNumber)?.floatValue ?? .nan
    try require(abs(restored - 90) < 0.001, "Upmixer crossover restore failed")
    try checkMetadata("restored")
    let output = try format(sampleRate: 48_000, channels: 10, tag: kAudioChannelLayoutTag_Atmos_5_1_4)
    try unit.outputBusses[0].setFormat(output)
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }
    try checkMetadata("recreated")
}

private func testUpmixerNativeRenderAllocation() throws {
    let inputFormat = try stereoFormat(sampleRate: 48_000)
    let layouts: [(AVAudioChannelCount, AudioChannelLayoutTag)] = [
        (10, kAudioChannelLayoutTag_Atmos_5_1_4),
        (12, kAudioChannelLayoutTag_Atmos_7_1_4),
    ]
    for (channels, tag) in layouts {
        let outputFormat = try format(sampleRate: 48_000, channels: channels, tag: tag)
        let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
        try unit.inputBusses[0].setFormat(inputFormat)
        try unit.outputBusses[0].setFormat(outputFormat)
        unit.maximumFramesToRender = 512
        try unit.allocateRenderResources()
        defer { unit.deallocateRenderResources() }
        guard let input = AVAudioPCMBuffer(pcmFormat: inputFormat, frameCapacity: 512),
              let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: 512),
              let gain = unit.parameterTree?.allParameters.first(where: {
                  $0.identifier == "height_gain"
              }) else {
            throw SmokeFailure.failure("cannot prepare Upmixer native render test")
        }
        input.frameLength = 512
        output.frameLength = 512
        for channel in 0..<2 {
            for frame in 0..<512 {
                input.floatChannelData![channel][frame] =
                    Float(0.1 + 0.05 * sin(Double(frame + channel * 7) * 0.11))
            }
        }
        for frames: AVAudioFrameCount in [1, 64, 128, 512] {
            input.frameLength = frames
            output.frameLength = frames
            for (automation, name) in [
                (SOTF_NATIVE_AUTOMATION_NONE, "static"),
                (SOTF_NATIVE_AUTOMATION_POINT, "point automation"),
                (SOTF_NATIVE_AUTOMATION_RAMP, "ramp automation"),
            ] {
                var allocations: UInt64 = 0
                var frees: UInt64 = 0
                let status = sotf_test_native_render(
                    unit, input.mutableAudioBufferList, output.mutableAudioBufferList,
                    frames, gain.address, automation, &allocations, &frees
                )
                if allocations != 0 || frees != 0 { sotfMallocCounterDumpCallers() }
                try require(status == noErr,
                            "Upmixer \(channels)ch/\(frames) \(name) render returned \(status)")
                try require(allocations == 0 && frees == 0,
                            "Upmixer \(channels)ch/\(frames) \(name) render allocated \(allocations) and freed \(frees)")
                try assertFiniteOverwrittenAndAudible(
                    output,
                    caseName: "Upmixer \(channels)ch/\(frames) native render",
                    allowedSilentChannels: Set(2..<Int(channels))
                )
            }
        }
    }
}

private func testDownmixLayoutMatrix() throws {
    let cases = [
        DownmixLayoutCase(name: "2.0", channels: 2, tag: kAudioChannelLayoutTag_Stereo,
                          labels: ["L", "R"]),
        DownmixLayoutCase(name: "2.1", channels: 3, tag: discreteTag(3),
                          labels: ["L", "R", "LFE"]),
        DownmixLayoutCase(name: "5.0", channels: 5, tag: kAudioChannelLayoutTag_MPEG_5_0_A,
                          labels: ["FL", "FR", "C", "SL", "SR"]),
        DownmixLayoutCase(name: "5.1", channels: 6, tag: kAudioChannelLayoutTag_MPEG_5_1_A,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR"]),
        DownmixLayoutCase(name: "7.1", channels: 8, tag: kAudioChannelLayoutTag_MPEG_7_1_C,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR"]),
        DownmixLayoutCase(name: "5.1.2", channels: 8, tag: kAudioChannelLayoutTag_Atmos_5_1_2,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "TML", "TMR"],
                          configurationID: "coreaudio_atmos_5.1.2"),
        DownmixLayoutCase(name: "5.1.4", channels: 10, tag: kAudioChannelLayoutTag_Atmos_5_1_4,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "TFL", "TFR", "TBL", "TBR"]),
        DownmixLayoutCase(name: "7.1.2", channels: 10, tag: kAudioChannelLayoutTag_Atmos_7_1_2,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR", "TML", "TMR"],
                          configurationID: "coreaudio_atmos_7.1.2"),
        DownmixLayoutCase(name: "7.1.4", channels: 12, tag: kAudioChannelLayoutTag_Atmos_7_1_4,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR", "TFL", "TFR", "TBL", "TBR"]),
        DownmixLayoutCase(name: "9.1.4", channels: 14, tag: discreteTag(14),
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR", "WL", "WR", "TFL", "TFR", "TBL", "TBR"]),
        DownmixLayoutCase(name: "9.1.6", channels: 16, tag: discreteTag(16),
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR", "WL", "WR", "TFL", "TFR", "TBL", "TBR", "TMiL", "TMiR"]),
        DownmixLayoutCase(name: "Core Audio 9.1.6", channels: 16,
                          tag: kAudioChannelLayoutTag_Atmos_9_1_6,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR", "WL", "WR", "TFL", "TFR", "TML", "TMR", "TBL", "TBR"],
                          configurationID: "coreaudio_atmos_9.1.6"),
    ]
    let sampleRate = 48_000.0
    let outputFormat = try stereoFormat(sampleRate: sampleRate)
    var routingSignatures: [String: [(Double, Double)]] = [:]

    for layoutCase in cases {
        let inputFormat = try format(
            sampleRate: sampleRate,
            channels: layoutCase.channels,
            tag: layoutCase.tag
        )
        let unit = try DownmixAudioUnit(
            componentDescription: componentDescription("SODm")
        )
        let configuration = unit.pluginConfiguration(
            inputFormat: inputFormat,
            outputFormat: outputFormat
        )
        let expectedConfigurationID = layoutCase.configurationID ?? layoutCase.name
        try require(
            configuration.contains("\"input_layout\":\"\(expectedConfigurationID)\""),
            "\(layoutCase.name): configuration lost Core Audio layout identity: \(configuration)"
        )
        try require(
            unit.channelCapabilities == downmixCapabilities,
            "\(layoutCase.name): Downmix capability table is not truthful"
        )
        let (_, output) = try render(
            unit,
            inputFormat: inputFormat,
            outputFormat: outputFormat,
            frames: 4096
        )
        try require(unit.hasRustPlugin, "\(layoutCase.name): Rust plugin did not instantiate")
        try assertFiniteAudibleStereo(output, caseName: layoutCase.name)

        var signature: [(Double, Double)] = []
        for channel in 0..<Int(layoutCase.channels) {
            let probe = try DownmixAudioUnit(
                componentDescription: componentDescription("SODm")
            )
            let (_, channelOutput) = try render(
                probe,
                inputFormat: inputFormat,
                outputFormat: outputFormat,
                frames: 4096,
                activeInputChannel: channel
            )
            let energy = try stereoEnergy(channelOutput)
            try require(
                energy.0 + energy.1 > 1.0e-8,
                "\(layoutCase.name): input channel \(channel) was dropped"
            )
            let label = layoutCase.labels[channel]
            if label == "C" || label == "LFE" {
                let balance = abs(energy.0 - energy.1) / max(energy.0 + energy.1, 1.0e-12)
                try require(
                    balance < 1.0e-4,
                    "\(layoutCase.name) \(label): center route is not balanced"
                )
            } else if label.hasSuffix("L") {
                try require(
                    energy.0 > energy.1 * 1.1,
                    "\(layoutCase.name) \(label): expected left-dominant route, got \(energy)"
                )
            } else if label.hasSuffix("R") {
                try require(
                    energy.1 > energy.0 * 1.1,
                    "\(layoutCase.name) \(label): expected right-dominant route, got \(energy)"
                )
            }
            signature.append(energy)
        }
        routingSignatures[layoutCase.name] = signature

        if layoutCase.name == "5.1.2" {
            let topMiddleLeft = signature[6]
            let topMiddleRight = signature[7]
            try require(
                topMiddleLeft.1 < topMiddleLeft.0 * 1.0e-4
                    && topMiddleRight.0 < topMiddleRight.1 * 1.0e-4,
                "Core Audio TopMiddle90 channels did not pan to their matching side"
            )
        }
        if layoutCase.name == "Core Audio 9.1.6" {
            let topMiddleLeft = signature[12]
            let topBackLeft = signature[14]
            let expectedRightToLeft = pow(tan(Double.pi / 8.0), 2.0)
            let observedRightToLeft = topBackLeft.1 / max(topBackLeft.0, 1.0e-12)
            try require(
                topMiddleLeft.1 < topMiddleLeft.0 * 1.0e-4
                    && abs(observedRightToLeft - expectedRightToLeft) < 0.03,
                "Core Audio 9.1.6 top-middle/back channel order or pan geometry changed"
            )
        }
    }


    for (first, second) in [("7.1", "5.1.2"), ("5.1.4", "7.1.2")] {
        guard let firstSignature = routingSignatures[first],
              let secondSignature = routingSignatures[second] else {
            throw SmokeFailure.failure("missing routing signature for \(first)/\(second)")
        }
        let delta = zip(firstSignature, secondSignature).reduce(0.0) { total, pair in
            total + abs(pair.0.0 - pair.1.0) + abs(pair.0.1 - pair.1.1)
        }
        try require(delta > 1.0e-6, "\(first) and \(second) routed identically")
    }
}

private func testMonoToStereoConstructionAndRender() throws {
    let sampleRate = 48_000.0
    guard let mono = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 1) else {
        throw SmokeFailure.failure("cannot construct mono format")
    }
    let stereo = try stereoFormat(sampleRate: sampleRate)
    let unit = try MonoToStereoAudioUnit(
        componentDescription: componentDescription("SOM2")
    )
    try require(unit.hasRustPlugin, "Mono-to-Stereo provisional 1→2 plugin missing")
    let (_, output) = try render(
        unit,
        inputFormat: mono,
        outputFormat: stereo,
        frames: 4096
    )
    try assertFiniteAudibleStereo(output, caseName: "Mono-to-Stereo")
}

private func testRepeatedConstructorFailureIsDeterministic() throws {
    for _ in 0..<1024 {
        do {
            _ = try InvalidAudioUnit(componentDescription: componentDescription("SOBD"))
            throw SmokeFailure.failure("invalid AU constructor unexpectedly succeeded")
        } catch let error as NSError {
            try require(
                error.code == Int(kAudioUnitErr_FormatNotSupported),
                "invalid AU constructor returned unexpected error \(error)"
            )
        }
    }
}

private func testDownmixFormatChangeAndUnsupportedLayout() throws {
    let sampleRate = 48_000.0
    let outputFormat = try stereoFormat(sampleRate: sampleRate)
    let unit = try DownmixAudioUnit(
        componentDescription: componentDescription("SODm")
    )

    for layoutCase in [
        DownmixLayoutCase(name: "5.1", channels: 6, tag: kAudioChannelLayoutTag_MPEG_5_1_A,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR"]),
        DownmixLayoutCase(name: "7.1.4", channels: 12, tag: kAudioChannelLayoutTag_Atmos_7_1_4,
                          labels: ["FL", "FR", "C", "LFE", "SL", "SR", "BL", "BR", "TFL", "TFR", "TBL", "TBR"]),
    ] {
        let inputFormat = try format(
            sampleRate: sampleRate,
            channels: layoutCase.channels,
            tag: layoutCase.tag
        )
        let (_, output) = try render(
            unit,
            inputFormat: inputFormat,
            outputFormat: outputFormat,
            frames: 4096
        )
        try require(unit.hasRustPlugin, "\(layoutCase.name): format change lost plugin")
        try assertFiniteAudibleStereo(output, caseName: "format change to \(layoutCase.name)")
    }

    func routingSignature(
        _ inputFormat: AVAudioFormat
    ) throws -> [(Double, Double)] {
        var signature: [(Double, Double)] = []
        for channel in 0..<Int(inputFormat.channelCount) {
            let (_, output) = try render(
                unit,
                inputFormat: inputFormat,
                outputFormat: outputFormat,
                frames: 4096,
                activeInputChannel: channel
            )
            signature.append(try stereoEnergy(output))
        }
        return signature
    }

    let sevenOne = try format(
        sampleRate: sampleRate,
        channels: 8,
        tag: kAudioChannelLayoutTag_MPEG_7_1_C
    )
    let fiveOneTwo = try format(
        sampleRate: sampleRate,
        channels: 8,
        tag: kAudioChannelLayoutTag_Atmos_5_1_2
    )
    let sevenOneSignature = try routingSignature(sevenOne)
    let fiveOneTwoSignature = try routingSignature(fiveOneTwo)
    let sameWidthDelta = zip(sevenOneSignature, fiveOneTwoSignature).reduce(0.0) {
        $0 + abs($1.0.0 - $1.1.0) + abs($1.0.1 - $1.1.1)
    }
    try require(
        sameWidthDelta > 1.0e-6,
        "same-instance 7.1→5.1.2 retained stale routing"
    )

    let heightParameter = unit.parameterTree?.allParameters.first {
        $0.identifier == "height_gain_db"
    }
    heightParameter?.setValue(-6.0, originator: nil)
    let retainedHeightGain = unit.parameterValueForTesting(identifier: "height_gain_db") ?? .nan
    let retainedInputWidth = unit.inputBusses[0].format.channelCount
    let retainedInputTag = unit.inputBusses[0].format.channelLayout?.layoutTag
    let retainedOutputWidth = unit.outputBusses[0].format.channelCount
    let unsupportedNamedLayout = try format(
        sampleRate: sampleRate,
        channels: 6,
        tag: kAudioChannelLayoutTag_MPEG_5_1_B
    )
    try require(
        !unit.shouldChange(to: unsupportedNamedLayout, for: unit.inputBusses[0]),
        "reordered MPEG 5.1 B was advertised as supported"
    )
    let unsupportedConfiguration = unit.pluginConfiguration(
        inputFormat: unsupportedNamedLayout,
        outputFormat: outputFormat
    )
    try require(
        unsupportedConfiguration == "{invalid}",
        "reordered MPEG 5.1 B did not fail closed"
    )
    do {
        try unit.inputBusses[0].setFormat(unsupportedNamedLayout)
        throw SmokeFailure.failure("reordered MPEG 5.1 B was accepted")
    } catch let error as NSError {
        try require(
            error.code == Int(kAudioUnitErr_FormatNotSupported),
            "reordered MPEG 5.1 B returned unexpected error \(error)"
        )
    }
    try require(
        unit.hasRustPlugin
            && unit.inputBusses[0].format.channelCount == retainedInputWidth
            && unit.inputBusses[0].format.channelLayout?.layoutTag == retainedInputTag
            && unit.outputBusses[0].format.channelCount == retainedOutputWidth,
        "unsupported named layout changed the live plugin or bus formats"
    )
    let heightGainAfterRefusal = unit.parameterValueForTesting(identifier: "height_gain_db") ?? .nan
    try require(
        abs(Double(heightGainAfterRefusal - retainedHeightGain)) < 1.0e-4,
        "unsupported named layout changed Downmix parameter state"
    )

    let surroundOutput = try format(
        sampleRate: sampleRate,
        channels: 6,
        tag: kAudioChannelLayoutTag_MPEG_5_1_A
    )
    do {
        try unit.inputBusses[0].setFormat(surroundOutput)
        try unit.outputBusses[0].setFormat(surroundOutput)
        try unit.allocateRenderResources()
        unit.deallocateRenderResources()
        throw SmokeFailure.failure("non-stereo Downmix output was accepted")
    } catch let error as NSError {
        try require(
            error.code == Int(kAudioUnitErr_FormatNotSupported),
            "wrong output width returned unexpected error \(error)"
        )
    }

    let ambiguousEightChannel = try format(
        sampleRate: sampleRate,
        channels: 8,
        tag: discreteTag(8)
    )
    for (name, incompatible) in [
        ("AudioUnit 6.0", try format(
            sampleRate: sampleRate,
            channels: 6,
            tag: kAudioChannelLayoutTag_AudioUnit_6_0
        )),
        ("MPEG 5.1 B", try format(
            sampleRate: sampleRate,
            channels: 6,
            tag: kAudioChannelLayoutTag_MPEG_5_1_B
        )),
        ("discrete 12ch", try format(
            sampleRate: sampleRate,
            channels: 12,
            tag: discreteTag(12)
        )),
    ] {
        try require(
            !unit.shouldChange(to: incompatible, for: unit.inputBusses[0]),
            "\(name) was advertised as a supported Downmix input format"
        )
        let configuration = unit.pluginConfiguration(
            inputFormat: incompatible,
            outputFormat: outputFormat
        )
        try require(
            configuration == "{invalid}",
            "\(name) did not fail closed as a named input layout: \(configuration)"
        )
    }
    do {
        try unit.inputBusses[0].setFormat(ambiguousEightChannel)
        try unit.outputBusses[0].setFormat(outputFormat)
        try unit.allocateRenderResources()
        unit.deallocateRenderResources()
        throw SmokeFailure.failure("ambiguous 8-channel layout was accepted")
    } catch let error as NSError {
        try require(
            error.code == Int(kAudioUnitErr_FailedInitialization)
                || error.code == Int(kAudioUnitErr_FormatNotSupported),
            "ambiguous layout returned unexpected error \(error)"
        )
        try require(unit.hasRustPlugin, "failed layout destroyed the last valid Rust plugin")
    }

    let (_, sameWidthRecovered) = try render(
        unit,
        inputFormat: fiveOneTwo,
        outputFormat: outputFormat,
        frames: 4096
    )
    try assertFiniteAudibleStereo(
        sameWidthRecovered,
        caseName: "same-width post-rejection recovery"
    )

    let recoveryFormat = try format(
        sampleRate: sampleRate,
        channels: 6,
        tag: kAudioChannelLayoutTag_MPEG_5_1_A
    )
    let (_, recoveredOutput) = try render(
        unit,
        inputFormat: recoveryFormat,
        outputFormat: outputFormat,
        frames: 4096
    )
    try require(unit.hasRustPlugin, "valid format did not recover after rejection")
    try assertFiniteAudibleStereo(recoveredOutput, caseName: "post-rejection recovery")
}

private func testTaggedAtmosOrderRefusalPreservesWrapperState() throws {
    let constructors: [(String, () throws -> GenericRustAudioUnit)] = [
        ("AAE", { try AAEAudioUnit(componentDescription: componentDescription("SOAE")) }),
        ("Ambisonics", { try AmbisonicsAudioUnit(componentDescription: componentDescription("SOAm")) }),
        ("Upmixer", { try UpmixerAudioUnit(componentDescription: componentDescription("SOUp")) }),
    ]
    let unsupported = try format(
        sampleRate: 48_000,
        channels: 8,
        tag: kAudioChannelLayoutTag_Atmos_5_1_2
    )

    for (name, construct) in constructors {
        let unit = try construct()
        let inputWidth = unit.inputBusses[0].format.channelCount
        let outputWidth = unit.outputBusses[0].format.channelCount
        guard let parameter = unit.parameterTree?.allParameters.first else {
            throw SmokeFailure.failure("\(name): no parameter state to preserve")
        }
        let marker = (parameter.minValue + parameter.maxValue) * 0.5
        parameter.setValue(marker, originator: nil)
        let markerBefore = unit.parameterValueForTesting(identifier: parameter.identifier) ?? .nan
        try require(unit.hasRustPlugin, "\(name): provisional Rust plugin is missing")
        try require(
            !unit.shouldChange(to: unsupported, for: unit.outputBusses[0]),
            "\(name): mismatched Atmos 5.1.2 output order was advertised"
        )
        let configuration = unit.pluginConfiguration(
            inputFormat: unit.inputBusses[0].format,
            outputFormat: unsupported
        )
        try require(configuration == "{invalid}", "\(name): mismatched layout was accepted")
        let markerAfter = unit.parameterValueForTesting(identifier: parameter.identifier) ?? .nan
        try require(
            unit.hasRustPlugin
                && unit.inputBusses[0].format.channelCount == inputWidth
                && unit.outputBusses[0].format.channelCount == outputWidth
                && abs(Double(markerAfter - markerBefore)) < 1.0e-4,
            "\(name): refused named layout changed plugin, state, or buses"
        )
    }
}

private func testParameterStateSurvivesFormatRecreation() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let format48 = try stereoFormat(sampleRate: 48_000)
    _ = try render(
        unit,
        inputFormat: format48,
        outputFormat: format48,
        frames: 257
    )
    guard let gain = unit.parameterTree?.allParameters.first(where: {
        $0.identifier == "gain_db"
    }) else {
        throw SmokeFailure.failure("Gain AU did not publish gain_db")
    }
    gain.setValue(6.0, originator: nil)
    let queuedState = unit.parameterQueueStateForTesting()
    try require(
        queuedState.publishedEpoch == queuedState.renderEpoch,
        "Gain observer published into a stale epoch: \(queuedState)"
    )
    let observedValue = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(
        abs(observedValue - 6.0) < 1.0e-4,
        "Gain observer did not reach the live Rust instance: \(observedValue)"
    )
    let format96 = try stereoFormat(sampleRate: 96_000)
    _ = try render(
        unit,
        inputFormat: format96,
        outputFormat: format96,
        frames: 257
    )
    let recreatedValue = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(
        abs(recreatedValue - 6.0) < 1.0e-4,
        "Gain parameter reset across sample-rate recreation: \(recreatedValue)"
    )

    let retainedOldGain = gain
    unit.rebuildParameterTreeForTesting()
    guard let replacementGain = unit.parameterTree?.allParameters.first(where: {
        $0.identifier == "gain_db"
    }) else {
        throw SmokeFailure.failure("Gain replacement schema did not publish gain_db")
    }
    try require(
        replacementGain !== retainedOldGain,
        "explicit schema replacement retained the old AUParameter object"
    )
    replacementGain.setValue(-6, originator: nil)
    _ = unit.parameterValueForTesting(identifier: "gain_db")
    retainedOldGain.setValue(20, originator: nil)
    _ = try render(unit, inputFormat: format96, outputFormat: format96, frames: 257)
    let valueAfterStaleWrite = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(
        abs(valueAfterStaleWrite - (-6)) < 0.01,
        "retained old AUParameter wrote into replacement schema: \(valueAfterStaleWrite)"
    )
}

private func testMigrationFailureIsTransactional() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo48 = try stereoFormat(sampleRate: 48_000)
    _ = try render(unit, inputFormat: stereo48, outputFormat: stereo48, frames: 64)
    guard let gain = unit.parameterTree?.allParameters.first(where: {
        $0.identifier == "gain_db"
    }) else {
        throw SmokeFailure.failure("migration failure test is missing gain_db")
    }
    gain.value = 6.0
    unit.injectMigrationStateForTesting(Data("not valid plugin state".utf8))
    let stereo96 = try stereoFormat(sampleRate: 96_000)
    try unit.inputBusses[0].setFormat(stereo96)
    try unit.outputBusses[0].setFormat(stereo96)
    do {
        try unit.allocateRenderResources()
        unit.deallocateRenderResources()
        throw SmokeFailure.failure("invalid migration state replaced the valid plugin")
    } catch let error as NSError {
        try require(
            error.code == Int(kAudioUnitErr_FailedInitialization),
            "migration failure returned \(error)"
        )
    }
    try require(unit.hasRustPlugin, "migration failure destroyed the old plugin")
    let valueAfterFailure = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(abs(valueAfterFailure - 6.0) < 1.0e-4, "migration failure changed old parameters")
    let (_, recovered) = try render(
        unit,
        inputFormat: stereo48,
        outputFormat: stereo48,
        frames: 64
    )
    try assertFiniteOverwrittenAndAudible(recovered, caseName: "migration failure recovery")
    let valueAfterRecovery = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(abs(valueAfterRecovery - 6.0) < 1.0e-4, "recovery lost the old plugin state")
}

private func testTopologyMigrationUsesPortableParameters() throws {
    let unit = try ChannelMuteSoloAudioUnit(componentDescription: componentDescription("SOCs"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    _ = try render(unit, inputFormat: stereo, outputFormat: stereo, frames: 64)
    guard let dim = unit.parameterTree?.allParameters.first(where: {
        $0.identifier == "dim_gain_db"
    }) else {
        throw SmokeFailure.failure("ChannelMuteSolo is missing dim_gain_db")
    }
    dim.value = -9
    let surround = try plainFormat(sampleRate: 48_000, channels: 6)
    let (_, output) = try render(
        unit,
        inputFormat: surround,
        outputFormat: surround,
        frames: 64
    )
    try assertFiniteOverwrittenAndAudible(output, caseName: "topology migration")
    guard unit.parameterTree?.allParameters.contains(where: {
        $0.identifier == "dim_gain_db"
    }) == true else {
        throw SmokeFailure.failure("topology migration lost parameter tree")
    }
    let migratedValue = unit.parameterValueForTesting(identifier: "dim_gain_db") ?? .nan
    try require(abs(migratedValue - (-9)) < 1.0e-4, "portable scalar was not migrated")
}

private func testFullStatePartialFailureIsTransactional() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    _ = try render(unit, inputFormat: stereo, outputFormat: stereo, frames: 64)
    guard let gain = unit.parameterTree?.allParameters.first(where: {
        $0.identifier == "gain_db"
    }) else {
        throw SmokeFailure.failure("transactional fullState test is missing gain_db")
    }
    gain.value = 0
    _ = try render(unit, inputFormat: stereo, outputFormat: stereo, frames: 64)
    unit.fullState = [
        "sotf_state": Data(#"{"gain_db":6.0,"smoothing_ms":"invalid"}"#.utf8)
    ]
    let valueAfterFailure = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(abs(valueAfterFailure) < 1.0e-4, "failed fullState partially mutated the live plugin")
}

private func testFullStateRestoresHostParameterMap() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    _ = try render(unit, inputFormat: stereo, outputFormat: stereo, frames: 64)
    guard var state = unit.fullState else {
        throw SmokeFailure.failure("fullState parameter-map test could not save state")
    }
    // The serialized plugin blob and host parameter dictionary are distinct
    // contracts. Make the dictionary authoritative and prove it is restored
    // transactionally with the blob.
    state["gain_db"] = NSNumber(value: 6.0)
    unit.fullState = state
    let restored = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(abs(restored - 6.0) < 1.0e-4, "fullState discarded host parameter-map value")
}

private func testResourcePreparationFailureRollback() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    _ = try render(unit, inputFormat: stereo, outputFormat: stereo, frames: 64)
    for stage in 1...4 {
        unit.injectAllocationFailureForTesting(stage: stage)
        try unit.inputBusses[0].setFormat(stereo)
        try unit.outputBusses[0].setFormat(stereo)
        do {
            try unit.allocateRenderResources()
            unit.deallocateRenderResources()
            throw SmokeFailure.failure("resource failure stage \(stage) unexpectedly succeeded")
        } catch let error as NSError {
            try require(
                error.code == Int(kAudioUnitErr_FailedInitialization),
                "resource failure stage \(stage) returned \(error)"
            )
        }
        try require(unit.hasRustPlugin, "resource failure stage \(stage) lost live plugin")
        let (_, recovered) = try render(
            unit, inputFormat: stereo, outputFormat: stereo, frames: 64
        )
        try assertFiniteOverwrittenAndAudible(
            recovered, caseName: "resource failure stage \(stage) recovery"
        )
    }
}

private func testRetainedRenderBlockOwnsInputStorage() throws {
    let stereo = try stereoFormat(sampleRate: 48_000)
    var unit: GainAudioUnit? = try GainAudioUnit(
        componentDescription: componentDescription("SOGN")
    )
    try unit?.inputBusses[0].setFormat(stereo)
    try unit?.outputBusses[0].setFormat(stereo)
    unit?.maximumFramesToRender = 64
    try unit?.allocateRenderResources()
    guard let block = unit?.internalRenderBlock,
          let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 64),
          let output = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 64) else {
        throw SmokeFailure.failure("retained-block test setup failed")
    }
    input.frameLength = 64
    output.frameLength = 64
    let pull: AURenderPullInputBlock = { _, _, _, _, destination in
        let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
        let target = UnsafeMutableAudioBufferListPointer(destination)
        guard source.count == target.count else { return kAudioUnitErr_InvalidPropertyValue }
        for index in 0..<source.count {
            memcpy(target[index].mData, source[index].mData, Int(source[index].mDataByteSize))
            target[index].mDataByteSize = source[index].mDataByteSize
        }
        return noErr
    }
    var flags = AudioUnitRenderActionFlags(rawValue: 0)
    var timestamp = AudioTimeStamp()
    unit?.deallocateRenderResources()
    let deallocatedStatus = block(
        &flags, &timestamp, 64, 0, output.mutableAudioBufferList, nil, pull
    )
    try require(
        deallocatedStatus == kAudioUnitErr_Uninitialized,
        "deallocated retained block remained renderable: \(deallocatedStatus)"
    )
    try unit?.allocateRenderResources()
    weak let releasedUnit = unit
    unit = nil
    try require(releasedUnit == nil, "render block unexpectedly retained AUAudioUnit")
    let status = block(
        &flags, &timestamp, 64, 0, output.mutableAudioBufferList, nil, pull
    )
    try require(
        status == kAudioUnitErr_Uninitialized,
        "released AU retained block did not preserve the deallocated contract: \(status)"
    )
}

private func testStrictFormatNegotiationAndRecovery() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo48 = try stereoFormat(sampleRate: 48_000)
    guard let int16 = AVAudioFormat(
        commonFormat: .pcmFormatInt16,
        sampleRate: 48_000,
        channels: 2,
        interleaved: false
    ) else {
        throw SmokeFailure.failure("cannot construct Int16 rejection format")
    }
    do {
        try unit.inputBusses[0].setFormat(int16)
        throw SmokeFailure.failure("non-Float32 input format was accepted")
    } catch let error as NSError {
        try require(error.code == Int(kAudioUnitErr_FormatNotSupported), "Int16 rejection returned \(error)")
    }

    try unit.inputBusses[0].setFormat(stereo48)
    try unit.outputBusses[0].setFormat(try stereoFormat(sampleRate: 96_000))
    do {
        try unit.allocateRenderResources()
        unit.deallocateRenderResources()
        throw SmokeFailure.failure("mismatched input/output sample rates were accepted")
    } catch let error as NSError {
        try require(
            error.code == Int(kAudioUnitErr_FormatNotSupported),
            "sample-rate mismatch returned \(error)"
        )
    }

    let (_, recovered) = try render(
        unit,
        inputFormat: stereo48,
        outputFormat: stereo48,
        frames: 64
    )
    try assertFiniteOverwrittenAndAudible(recovered, caseName: "format rejection recovery")
}

private func testRenderBoundsAndOutputOwnership() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(stereo)
    unit.maximumFramesToRender = 64
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }

    guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 65),
          let output = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 65) else {
        throw SmokeFailure.failure("cannot allocate render-bound test buffers")
    }
    input.frameLength = 65
    output.frameLength = 65
    let pullInput: AURenderPullInputBlock = { _, _, requestedFrames, _, destination in
        guard requestedFrames <= input.frameLength else { return kAudioUnitErr_TooManyFramesToProcess }
        let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
        let target = UnsafeMutableAudioBufferListPointer(destination)
        guard source.count == target.count else { return kAudioUnitErr_InvalidPropertyValue }
        for index in 0..<source.count {
            guard let src = source[index].mData, let dst = target[index].mData else {
                return kAudioUnitErr_NoConnection
            }
            memcpy(dst, src, Int(source[index].mDataByteSize))
            target[index].mDataByteSize = source[index].mDataByteSize
        }
        return noErr
    }
    let block = unit.internalRenderBlock
    var flags = AudioUnitRenderActionFlags(rawValue: 0)
    var timestamp = AudioTimeStamp()
    let oversized = block(
        &flags, &timestamp, 65, 0, output.mutableAudioBufferList, nil, pullInput
    )
    try require(
        oversized == kAudioUnitErr_TooManyFramesToProcess,
        "oversized render returned \(oversized)"
    )

    let relaxedOutput = UnsafeMutableAudioBufferListPointer(output.mutableAudioBufferList)
    let savedOutputBytes = relaxedOutput[0].mDataByteSize
    relaxedOutput[0].mDataByteSize = 0
    flags = AudioUnitRenderActionFlags(rawValue: (1 << 9) | (1 << 4))
    let uncheckedStatus = block(
        &flags, &timestamp, 64, 0, output.mutableAudioBufferList, nil, pullInput
    )
    relaxedOutput[0].mDataByteSize = savedOutputBytes
    try require(
        uncheckedStatus == noErr,
        "DoNotCheckRenderArgs did not bypass trusted byte-size validation: \(uncheckedStatus)"
    )
    try require(
        flags.rawValue & (1 << 4) == 0,
        "render retained a stale OutputIsSilence claim"
    )
    flags = AudioUnitRenderActionFlags(rawValue: 0)

    var nilOutput = AudioBufferList(
        mNumberBuffers: 1,
        mBuffers: AudioBuffer(mNumberChannels: 2, mDataByteSize: 512, mData: nil)
    )
    let nilStatus = withUnsafeMutablePointer(to: &nilOutput) { pointer in
        sotfRenderCounterArm()
        let status = block(&flags, &timestamp, 64, 0, pointer, nil, pullInput)
        var allocations: UInt64 = 0
        var frees: UInt64 = 0
        sotfRenderCounterDisarm(&allocations, &frees)
        if allocations != 0 || frees != 0 {
            return kAudioUnitErr_FailedInitialization
        }
        return status
    }
    try require(
        nilStatus == noErr,
        "AU-owned nil output storage render failed: \(nilStatus)"
    )
    try require(
        nilOutput.mBuffers.mData != nil
            && nilOutput.mBuffers.mDataByteSize == 64 * 2 * UInt32(MemoryLayout<Float>.size),
        "AU did not install correctly sized owned interleaved output storage"
    )
    let ownedSamples = nilOutput.mBuffers.mData!.assumingMemoryBound(to: Float.self)
    try require(
        (0..<(64 * 2)).allSatisfy { ownedSamples[$0].isFinite },
        "AU-owned interleaved output contained non-finite samples"
    )

    var shortStorage = [Float](repeating: .nan, count: 64)
    let shortStatus = shortStorage.withUnsafeMutableBytes { bytes -> OSStatus in
        var shortOutput = AudioBufferList(
            mNumberBuffers: 1,
            mBuffers: AudioBuffer(
                mNumberChannels: 1,
                mDataByteSize: UInt32(bytes.count),
                mData: bytes.baseAddress
            )
        )
        return withUnsafeMutablePointer(to: &shortOutput) { pointer in
            block(&flags, &timestamp, 64, 0, pointer, nil, pullInput)
        }
    }
    try require(
        shortStatus == kAudioUnitErr_InvalidPropertyValue,
        "short output buffer list was reported successful: \(shortStatus)"
    )
    unit.deallocateRenderResources()
    try require(
        !unit.retainsRenderStorageForTesting(),
        "deallocateRenderResources retained render-only storage"
    )
}

private func testThirtyTwoChannelCapsAndRecovery() throws {
    let cases: [(String, () throws -> GenericRustAudioUnit)] = [
        ("LoudnessCompensation", {
            try LoudnessCompensationAudioUnit(componentDescription: componentDescription("SOLc"))
        }),
        ("FletcherMunson", {
            try FletcherMunsonAudioUnit(componentDescription: componentDescription("SOFm"))
        }),
        ("Saturation", {
            try SaturationAudioUnit(componentDescription: componentDescription("SOSt"))
        }),
    ]
    let format32 = try plainFormat(sampleRate: 48_000, channels: 32)
    let format33 = try plainFormat(sampleRate: 48_000, channels: 33)
    let stereo = try stereoFormat(sampleRate: 48_000)
    for (name, construct) in cases {
        let unit = try construct()
        try require(unit.inputBusses[0].maximumChannelCount == 32, "\(name) input cap is not 32")
        try require(unit.outputBusses[0].maximumChannelCount == 32, "\(name) output cap is not 32")
        let renderFrames = name == "Saturation"
            ? max(64, Int(ceil(unit.latency * 48_000)) + 64)
            : 64
        let (_, output32) = try render(
            unit,
            inputFormat: format32,
            outputFormat: format32,
            frames: AVAudioFrameCount(renderFrames)
        )
        try assertFiniteOverwrittenAndAudible(output32, caseName: "\(name) 32ch")
        do {
            try unit.inputBusses[0].setFormat(format33)
            throw SmokeFailure.failure("\(name) accepted 33 input channels")
        } catch let error as NSError {
            try require(error.code == Int(kAudioUnitErr_FormatNotSupported), "\(name) 33ch returned \(error)")
        }
        let (_, recovered) = try render(
            unit,
            inputFormat: stereo,
            outputFormat: stereo,
            frames: AVAudioFrameCount(renderFrames)
        )
        try assertFiniteOverwrittenAndAudible(recovered, caseName: "\(name) cap recovery")
    }
}

private func testParameterQueueContentionOrderAndOverflow() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(stereo)
    unit.maximumFramesToRender = 64
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }
    guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 64),
          let output = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 64),
          let gain = unit.parameterTree?.allParameters.first(where: {
              $0.identifier == "gain_db"
          }) else {
        throw SmokeFailure.failure("cannot prepare parameter mailbox test")
    }
    input.frameLength = 64
    output.frameLength = 64
    let inputBufferList = input.mutableAudioBufferList
    let pullInput: AURenderPullInputBlock = { _, _, _, _, destination in
        let source = UnsafeMutableAudioBufferListPointer(inputBufferList)
        let target = UnsafeMutableAudioBufferListPointer(destination)
        guard source.count == target.count else { return kAudioUnitErr_InvalidPropertyValue }
        for index in 0..<source.count {
            guard let src = source[index].mData, let dst = target[index].mData else {
                return kAudioUnitErr_NoConnection
            }
            memcpy(dst, src, Int(source[index].mDataByteSize))
            target[index].mDataByteSize = source[index].mDataByteSize
        }
        return noErr
    }
    let block = unit.internalRenderBlock
    var flags = AudioUnitRenderActionFlags(rawValue: 0)
    var timestamp = AudioTimeStamp()
    func renderOnce() -> OSStatus {
        block(&flags, &timestamp, 64, 0, output.mutableAudioBufferList, nil, pullInput)
    }

    let gateHeld = DispatchSemaphore(value: 0)
    let releaseGate = DispatchSemaphore(value: 0)
    let gateDone = DispatchSemaphore(value: 0)
    Thread.detachNewThread {
        unit.holdRenderAccessForTesting {
            gateHeld.signal()
            _ = releaseGate.wait(timeout: .now() + 10)
        }
        gateDone.signal()
    }
    try require(gateHeld.wait(timeout: .now() + 10) == .success, "render gate was not held")
    gain.setValue(-3, originator: nil)
    gain.setValue(-6, originator: nil)
    gain.setValue(-9, originator: nil)
    releaseGate.signal()
    try require(gateDone.wait(timeout: .now() + 10) == .success, "render gate did not release")
    // A later producer must join the same sequence rather than overtaking the
    // older queued commands through a direct handle write.
    gain.setValue(-12, originator: nil)
    try require(renderOnce() == noErr, "queued-order render failed")
    let orderedGain = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(
        abs(orderedGain - (-12)) < 0.01,
        "mixed queued/observer parameters were not applied in producer order (gain=\(orderedGain))"
    )
    let overflowGateHeld = DispatchSemaphore(value: 0)
    let releaseOverflowGate = DispatchSemaphore(value: 0)
    let overflowGateDone = DispatchSemaphore(value: 0)
    Thread.detachNewThread {
        unit.holdRenderAccessForTesting {
            overflowGateHeld.signal()
            _ = releaseOverflowGate.wait(timeout: .now() + 10)
        }
        overflowGateDone.signal()
    }
    try require(
        overflowGateHeld.wait(timeout: .now() + 10) == .success,
        "overflow render gate was not held"
    )
    let coalescedBefore = unit.coalescedParameterPublicationCount
    for index in 0..<300 {
        let normalized = Float(index % 101) / 100.0
        try require(
            unit.enqueueParameterForTesting(
                address: gain.address,
                normalizedValue: normalized
            ),
            "could not publish coalesced parameter value"
        )
    }
    try require(
        unit.coalescedParameterPublicationCount >= coalescedBefore + 299,
        "parameter mailbox did not report coalesced publications"
    )
    releaseOverflowGate.signal()
    try require(
        overflowGateDone.wait(timeout: .now() + 10) == .success,
        "overflow render gate did not release"
    )
    try require(renderOnce() == noErr, "overflow recovery render failed")
    let coalescedGain = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    let expectedCoalescedGain = gain.minValue + 0.97 * (gain.maxValue - gain.minValue)
    try require(
        abs(coalescedGain - expectedCoalescedGain) <= 0.5,
        "parameter mailbox did not retain the newest publication (gain=\(coalescedGain))"
    )

    // Parameter publication must not wait for the Rust/render ownership gate.
    let accessHeld = DispatchSemaphore(value: 0)
    let releaseAccess = DispatchSemaphore(value: 0)
    let accessDone = DispatchSemaphore(value: 0)
    Thread.detachNewThread {
        unit.holdRenderAccessForTesting {
            accessHeld.signal()
            _ = releaseAccess.wait(timeout: .now() + 10)
        }
        accessDone.signal()
    }
    try require(accessHeld.wait(timeout: .now() + 10) == .success, "render access was not held")
    let started = DispatchTime.now().uptimeNanoseconds
    sotfMallocCounterBegin()
    let pendingNormalized = Float((-18 - gain.minValue) / (gain.maxValue - gain.minValue))
    try require(
        unit.enqueueParameterForTesting(
            address: gain.address,
            normalizedValue: pendingNormalized
        ),
        "could not publish while render access was held"
    )
    var allocations: UInt64 = 0
    var frees: UInt64 = 0
    sotfMallocCounterEnd(&allocations, &frees)
    let elapsed = DispatchTime.now().uptimeNanoseconds - started
    try require(elapsed < 100_000_000, "parameter publication waited for \(elapsed) ns")
    try require(
        allocations == 0 && frees == 0,
        "parameter publication allocated: allocations=\(allocations), frees=\(frees)"
    )
    releaseAccess.signal()
    try require(accessDone.wait(timeout: .now() + 10) == .success, "render access did not release")
    try require(renderOnce() == noErr, "post-publication render failed")
    let appliedGain = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(
        abs(appliedGain - (-18)) < 0.01,
        "queued command was lost after contention (gain=\(appliedGain))"
    )
}

private func testSpeechDenoiserNegotiationBeforeAllocation() throws {
    let unit = try SpeechDenoiserAudioUnit(componentDescription: componentDescription("SOSd"))
    let mono48 = try plainFormat(sampleRate: 48_000, channels: 1)
    let stereo48 = try stereoFormat(sampleRate: 48_000)
    let surround48 = try plainFormat(sampleRate: 48_000, channels: 6)
    let stereo96 = try stereoFormat(sampleRate: 96_000)

    try require(unit.shouldChange(to: mono48, for: unit.inputBusses[0]), "SpeechDenoiser rejected mono 48 kHz input")
    try require(unit.shouldChange(to: stereo48, for: unit.outputBusses[0]), "SpeechDenoiser rejected stereo 48 kHz output")
    try require(!unit.shouldChange(to: surround48, for: unit.inputBusses[0]), "SpeechDenoiser advertised 6-channel input")
    try require(!unit.shouldChange(to: surround48, for: unit.outputBusses[0]), "SpeechDenoiser advertised 6-channel output")
    try require(!unit.shouldChange(to: stereo96, for: unit.inputBusses[0]), "SpeechDenoiser advertised 96 kHz input")
    try require(!unit.shouldChange(to: stereo96, for: unit.outputBusses[0]), "SpeechDenoiser advertised 96 kHz output")
}

private func testParameterRenderEventPointAndRampOffsets() throws {
    let blockStart: AUEventSampleTime = 1_024
    func run(event: inout AURenderEvent) throws -> ([Float], AUValue) {
        let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
        let stereo = try stereoFormat(sampleRate: 48_000)
        try unit.inputBusses[0].setFormat(stereo)
        try unit.outputBusses[0].setFormat(stereo)
        unit.maximumFramesToRender = 32
        try unit.allocateRenderResources()
        defer { unit.deallocateRenderResources() }
        guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 32),
              let output = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 32),
              let inputChannels = input.floatChannelData,
              let outputChannels = output.floatChannelData,
              let gain = unit.parameterTree?.allParameters.first(where: {
                  $0.identifier == "gain_db"
              }) else {
            let ids = unit.parameterTree?.allParameters.map(\.identifier) ?? []
            throw SmokeFailure.failure("cannot prepare parameter render-event test; ids=\(ids)")
        }
        input.frameLength = 32
        output.frameLength = 32
        let inputBufferList = input.mutableAudioBufferList
        for channel in 0..<2 {
            for frame in 0..<32 {
                inputChannels[channel][frame] = 0.25
                outputChannels[channel][frame] = .nan
            }
        }
        if event.head.eventType == .parameter || event.head.eventType == .parameterRamp {
            event.parameter.parameterAddress = gain.address
        }
        let pullInput: AURenderPullInputBlock = { _, _, _, _, destination in
            let source = UnsafeMutableAudioBufferListPointer(inputBufferList)
            let target = UnsafeMutableAudioBufferListPointer(destination)
            for index in 0..<source.count {
                guard let src = source[index].mData, let dst = target[index].mData else {
                    return kAudioUnitErr_NoConnection
                }
                memcpy(dst, src, Int(source[index].mDataByteSize))
                target[index].mDataByteSize = source[index].mDataByteSize
            }
            return noErr
        }
        var flags = AudioUnitRenderActionFlags(rawValue: 0)
        var timestamp = AudioTimeStamp()
        timestamp.mSampleTime = Float64(blockStart)
        timestamp.mFlags = .sampleTimeValid
        sotfRenderCounterArm()
        let status = withUnsafePointer(to: &event) { pointer in
            unit.internalRenderBlock(
                &flags,
                &timestamp,
                32,
                0,
                output.mutableAudioBufferList,
                pointer,
                pullInput
            )
        }
        var allocations: UInt64 = 0
        var frees: UInt64 = 0
        sotfRenderCounterDisarm(&allocations, &frees)
        if allocations != 0 || frees != 0 { sotfMallocCounterDumpCallers() }
        try require(
            allocations == 0 && frees == 0,
            "parameter event render allocated: allocations=\(allocations), frees=\(frees)"
        )
        try require(status == noErr, "parameter render event returned \(status)")
        return (
            Array(UnsafeBufferPointer(start: outputChannels[0], count: 32)),
            unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
        )
    }

    var pointParameter = AUParameterEvent()
    pointParameter.eventSampleTime = blockStart + 16
    pointParameter.eventType = .parameter
    pointParameter.value = 20
    var pointEvent = AURenderEvent(parameter: pointParameter)
    let (pointOutput, pointValue) = try run(event: &pointEvent)
    try require(pointOutput[..<16].allSatisfy { abs($0 - 0.25) < 0.001 }, "point event changed samples before offset")
    try require(pointOutput[16...].contains { $0 > 0.2501 }, "point event did not apply after offset")
    try require(abs(pointValue - 20) < 0.01, "point event did not publish its target")

    var rampParameter = AUParameterEvent()
    rampParameter.eventSampleTime = blockStart + 8
    rampParameter.eventType = .parameterRamp
    rampParameter.rampDurationSampleFrames = 8
    rampParameter.value = 20
    var rampEvent = AURenderEvent(parameter: rampParameter)
    let (rampOutput, rampValue) = try run(event: &rampEvent)
    try require(rampOutput[..<8].allSatisfy { abs($0 - 0.25) < 0.001 }, "ramp event changed samples before offset")
    try require(rampOutput[8...].contains { $0 > 0.2501 }, "ramp event did not apply over requested duration")
    try require(abs(rampValue - 20) < 0.01, "ramp event did not reach its target")

    var overridingPointParameter = AUParameterEvent()
    overridingPointParameter.eventSampleTime = blockStart + 8
    overridingPointParameter.eventType = .parameter
    overridingPointParameter.value = -60
    var overridingPoint = AURenderEvent(parameter: overridingPointParameter)
    var overriddenRampParameter = AUParameterEvent()
    overriddenRampParameter.eventSampleTime = blockStart + 8
    overriddenRampParameter.eventType = .parameterRamp
    overriddenRampParameter.rampDurationSampleFrames = 16
    overriddenRampParameter.value = 20
    var overriddenRamp = AURenderEvent(parameter: overriddenRampParameter)
    let (_, overrideValue) = try withUnsafeMutablePointer(to: &overridingPoint) { pointPointer in
        overriddenRamp.parameter.next = pointPointer
        return try run(event: &overriddenRamp)
    }
    try require(
        abs(overrideValue - (-60)) < 0.01,
        "same-offset point event did not override preceding ramp"
    )

    var midi = AUMIDIEvent()
    midi.eventSampleTime = blockStart + 4
    midi.eventType = .MIDI
    midi.length = 3
    midi.data = (0x90, 60, 100)
    var midiEvent = AURenderEvent(MIDI: midi)
    _ = try run(event: &midiEvent)
}

private func testParameterRampPersistsAcrossBlocks() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(stereo)
    unit.maximumFramesToRender = 32
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }
    guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 32),
          let output = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 32),
          let gain = unit.parameterTree?.allParameters.first(where: {
              $0.identifier == "gain_db"
          }) else {
        throw SmokeFailure.failure("cannot prepare cross-block ramp test")
    }
    input.frameLength = 32
    output.frameLength = 32
    let pull: AURenderPullInputBlock = { _, _, _, _, destination in
        let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
        let target = UnsafeMutableAudioBufferListPointer(destination)
        for index in 0..<source.count {
            memcpy(target[index].mData, source[index].mData, Int(source[index].mDataByteSize))
            target[index].mDataByteSize = source[index].mDataByteSize
        }
        return noErr
    }
    var parameter = AUParameterEvent()
    parameter.eventSampleTime = 8_192 + 16
    parameter.eventType = .parameterRamp
    parameter.rampDurationSampleFrames = 48
    parameter.parameterAddress = gain.address
    parameter.value = 20
    var event = AURenderEvent(parameter: parameter)
    var flags = AudioUnitRenderActionFlags(rawValue: 0)
    var timestamp = AudioTimeStamp()
    timestamp.mSampleTime = 8_192
    timestamp.mFlags = .sampleTimeValid
    let firstStatus = withUnsafePointer(to: &event) { pointer in
        unit.internalRenderBlock(
            &flags, &timestamp, 32, 0, output.mutableAudioBufferList, pointer, pull
        )
    }
    try require(firstStatus == noErr, "first ramp block failed: \(firstStatus)")
    let firstValue = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(
        firstValue > 0 && firstValue < 20,
        "long ramp was compressed into its first block: \(firstValue)"
    )
    timestamp.mSampleTime += 32
    let secondStatus = unit.internalRenderBlock(
        &flags, &timestamp, 32, 0, output.mutableAudioBufferList, nil, pull
    )
    try require(secondStatus == noErr, "second ramp block failed: \(secondStatus)")
    let secondValue = unit.parameterValueForTesting(identifier: "gain_db") ?? .nan
    try require(abs(secondValue - 20) < 0.01, "cross-block ramp missed target: \(secondValue)")
}

private func testUpmixerStatePreservesUnsteppedValues() throws {
    let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    let immersive = try format(sampleRate: 48_000, channels: 10, tag: kAudioChannelLayoutTag_Atmos_5_1_4)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(immersive)
    try unit.allocateRenderResources()
    unit.deallocateRenderResources()
    guard var incoming = unit.fullState, let data = incoming["sotf_state"] as? Data,
          var object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        throw SmokeFailure.failure("cannot capture Upmixer state for unstepped values")
    }
    let values: [String: Float] = [
        "gain_front_direct": 0.73,
        "voice_freq_min_hz": 537.3,
        "bass_gain_db": 0.13,
        "bass_cutoff_hz": 83.7,
        "height_gain": 0.731,
    ]
    for (identifier, value) in values {
        object[identifier] = value
        incoming.removeValue(forKey: identifier)
    }
    incoming["sotf_state"] = try JSONSerialization.data(withJSONObject: object)
    unit.fullState = incoming
    guard let saved = unit.fullState, let beforeData = saved["sotf_state"] as? Data,
          let before = try JSONSerialization.jsonObject(with: beforeData) as? [String: Any] else {
        throw SmokeFailure.failure("cannot read restored unstepped Upmixer state")
    }
    for (identifier, value) in values {
        guard let actual = (before[identifier] as? NSNumber)?.floatValue else {
            throw SmokeFailure.failure("missing restored Upmixer value \(identifier)")
        }
        try require(actual.bitPattern == value.bitPattern,
                    "initial state did not preserve \(identifier): \(actual) != \(value)")
    }
    unit.fullState = saved
    guard let afterState = unit.fullState, let afterData = afterState["sotf_state"] as? Data,
          let after = try JSONSerialization.jsonObject(with: afterData) as? [String: Any] else {
        throw SmokeFailure.failure("cannot read round-tripped Upmixer state")
    }
    var changes: [String] = []
    for (identifier, value) in values {
        guard let actual = (after[identifier] as? NSNumber)?.floatValue else {
            throw SmokeFailure.failure("missing round-tripped Upmixer value \(identifier)")
        }
        if actual.bitPattern != value.bitPattern { changes.append("\(identifier): \(value) -> \(actual)") }
    }
    try require(changes.isEmpty, "state round-trip changed unstepped values: \(changes.sorted())")

    guard let parameters = unit.parameterTree?.allParameters else {
        throw SmokeFailure.failure("missing Upmixer parameter tree")
    }
    let floatingParameters = parameters.compactMap { parameter -> (String, Float)? in
        guard unit.parameterIsFloatingPointForTesting(address: parameter.address) == true else { return nil }
        let value = parameter.minValue + (parameter.maxValue - parameter.minValue) * 0.438371
        return (parameter.identifier, value)
    }
    try require(floatingParameters.count == 37, "unexpected Upmixer float parameter count")
    for (identifier, value) in floatingParameters {
        var trial = saved
        var blob = before
        blob[identifier] = value
        trial.removeValue(forKey: identifier)
        trial["sotf_state"] = try JSONSerialization.data(withJSONObject: blob)
        unit.fullState = trial
        guard let captured = unit.fullState, let capturedData = captured["sotf_state"] as? Data,
              let capturedBlob = try JSONSerialization.jsonObject(with: capturedData) as? [String: Any],
              let accepted = (capturedBlob[identifier] as? NSNumber)?.floatValue,
              let original = (before[identifier] as? NSNumber)?.floatValue else {
            throw SmokeFailure.failure("cannot inspect Upmixer state for \(identifier)")
        }
        try require(accepted.bitPattern != original.bitPattern,
                    "test did not change Upmixer value \(identifier)")
        unit.fullState = captured
        guard let repeated = unit.fullState, let repeatedData = repeated["sotf_state"] as? Data,
              let repeatedBlob = try JSONSerialization.jsonObject(with: repeatedData) as? [String: Any] else {
            throw SmokeFailure.failure("cannot inspect repeated Upmixer state for \(identifier)")
        }
        for (key, beforeValue) in capturedBlob {
            guard let first = beforeValue as? NSNumber, let second = repeatedBlob[key] as? NSNumber else {
                throw SmokeFailure.failure("unexpected nonnumeric Upmixer state field \(key)")
            }
            try require(first.doubleValue == second.doubleValue,
                        "repeating \(identifier) state changed \(key): \(first) -> \(second)")
        }
    }

    var edited = saved
    edited["gain_front_direct"] = NSNumber(value: Float(1.23))
    edited["bass_gain_db"] = NSNumber(value: Float(0.77))
    unit.fullState = edited
    let changed = unit.parameterValueForTesting(identifier: "gain_front_direct") ?? .nan
    try require(abs(changed - 1.25) < 0.001, "host override did not retain stepped behavior: \(changed)")
    let changedBass = unit.parameterValueForTesting(identifier: "bass_gain_db") ?? .nan
    try require(abs(changedBass - 0.77) < 0.001, "host structural override was ignored: \(changedBass)")

    var repaired = saved
    var malformedBlob = before
    malformedBlob["bass_gain_db"] = false
    repaired["sotf_state"] = try JSONSerialization.data(withJSONObject: malformedBlob)
    repaired["bass_gain_db"] = NSNumber(value: Float(0))
    unit.fullState = repaired
    let repairedBass = unit.parameterValueForTesting(identifier: "bass_gain_db") ?? .nan
    try require(abs(repairedBass) < 0.001, "valid host override did not repair malformed bass state")
}

private func testUpmixerHeightRampPrecision() throws {
    let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    let immersive = try format(sampleRate: 48_000, channels: 10, tag: kAudioChannelLayoutTag_Atmos_5_1_4)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(immersive)
    unit.maximumFramesToRender = 32
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }
    guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 32),
          let output = AVAudioPCMBuffer(pcmFormat: immersive, frameCapacity: 32),
          let gain = unit.parameterTree?.allParameters.first(where: { $0.identifier == "height_gain" }) else {
        throw SmokeFailure.failure("cannot prepare precise Upmixer height ramp test")
    }
    input.frameLength = 32
    output.frameLength = 32
    let pull: AURenderPullInputBlock = { _, _, _, _, destination in
        let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
        let target = UnsafeMutableAudioBufferListPointer(destination)
        for index in 0..<source.count {
            memcpy(target[index].mData, source[index].mData, Int(source[index].mDataByteSize))
            target[index].mDataByteSize = source[index].mDataByteSize
        }
        return noErr
    }
    var parameter = AUParameterEvent()
    parameter.eventSampleTime = 8_192 + 16
    parameter.eventType = .parameterRamp
    parameter.rampDurationSampleFrames = 48
    parameter.parameterAddress = gain.address
    parameter.value = 1.49
    var event = AURenderEvent(parameter: parameter)
    var flags = AudioUnitRenderActionFlags(rawValue: 0)
    var timestamp = AudioTimeStamp()
    timestamp.mSampleTime = 8_192
    timestamp.mFlags = .sampleTimeValid
    let firstStatus = withUnsafePointer(to: &event) { pointer in
        unit.internalRenderBlock(&flags, &timestamp, 32, 0, output.mutableAudioBufferList, pointer, pull)
    }
    try require(firstStatus == noErr, "first precise height ramp block failed: \(firstStatus)")
    let first = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    let expected: Float = 1 + (1.49 - 1) / 3
    try require(abs(first - expected) < 0.001, "height ramp was quantized: \(first), expected \(expected)")
    timestamp.mSampleTime += 32
    let secondStatus = unit.internalRenderBlock(
        &flags, &timestamp, 32, 0, output.mutableAudioBufferList, nil, pull
    )
    try require(secondStatus == noErr, "second precise height ramp block failed: \(secondStatus)")
    let final = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(abs(final - 1.49) < 0.001, "height ramp missed precise target: \(final)")
    guard let saved = unit.fullState else { throw SmokeFailure.failure("cannot save precise height gain") }

    timestamp.mSampleTime += 32
    parameter.eventSampleTime = AUEventSampleTime(timestamp.mSampleTime)
    parameter.rampDurationSampleFrames = 96
    parameter.value = 0.37
    event = AURenderEvent(parameter: parameter)
    let newRampStatus = withUnsafePointer(to: &event) { pointer in
        unit.internalRenderBlock(&flags, &timestamp, 32, 0, output.mutableAudioBufferList, pointer, pull)
    }
    try require(newRampStatus == noErr, "second height ramp failed: \(newRampStatus)")
    let secondRamp = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    let secondExpected: Float = 1.49 + (0.37 - 1.49) / 3
    try require(abs(secondRamp - secondExpected) < 0.001, "second height ramp lost precision: \(secondRamp)")

    timestamp.mSampleTime += 32
    parameter.eventSampleTime = AUEventSampleTime(timestamp.mSampleTime)
    parameter.eventType = .parameter
    parameter.rampDurationSampleFrames = 0
    parameter.value = 0.73
    event = AURenderEvent(parameter: parameter)
    let pointStatus = withUnsafePointer(to: &event) { pointer in
        unit.internalRenderBlock(&flags, &timestamp, 32, 0, output.mutableAudioBufferList, pointer, pull)
    }
    try require(pointStatus == noErr, "height point event failed: \(pointStatus)")
    let point = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(abs(point - 0.75) < 0.001, "point event did not retain stepped behavior: \(point)")
    timestamp.mSampleTime += 32
    let afterPointStatus = unit.internalRenderBlock(
        &flags, &timestamp, 32, 0, output.mutableAudioBufferList, nil, pull
    )
    let afterPoint = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(afterPointStatus == noErr && abs(afterPoint - 0.75) < 0.001,
                "height ramp continued after point event: \(afterPoint)")
    unit.deallocateRenderResources()
    unit.fullState = saved
    let restored = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(abs(restored - 1.49) < 0.001, "state restore quantized height gain: \(restored)")
    let larger = try format(sampleRate: 48_000, channels: 12, tag: kAudioChannelLayoutTag_Atmos_7_1_4)
    try unit.outputBusses[0].setFormat(larger)
    try unit.allocateRenderResources()
    let reallocated = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(abs(reallocated - 1.49) < 0.001, "format migration changed height gain: \(reallocated)")
}

private func testUpmixerPreciseRampInterruption() throws {
    let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    let immersive = try format(sampleRate: 48_000, channels: 10, tag: kAudioChannelLayoutTag_Atmos_5_1_4)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(immersive)
    unit.maximumFramesToRender = 1
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }
    guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 1),
          let output = AVAudioPCMBuffer(pcmFormat: immersive, frameCapacity: 1),
          let gain = unit.parameterTree?.allParameters.first(where: { $0.identifier == "height_gain" }) else {
        throw SmokeFailure.failure("cannot prepare interrupted height ramp test")
    }
    input.frameLength = 1
    output.frameLength = 1
    let pull: AURenderPullInputBlock = { _, _, _, _, destination in
        let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
        let target = UnsafeMutableAudioBufferListPointer(destination)
        for index in 0..<source.count {
            memcpy(target[index].mData, source[index].mData, Int(source[index].mDataByteSize))
            target[index].mDataByteSize = source[index].mDataByteSize
        }
        return noErr
    }
    var parameter = AUParameterEvent()
    parameter.eventSampleTime = 10_000
    parameter.eventType = .parameterRamp
    parameter.rampDurationSampleFrames = 64
    parameter.parameterAddress = gain.address
    parameter.value = 1.49
    var event = AURenderEvent(parameter: parameter)
    var flags = AudioUnitRenderActionFlags(rawValue: 0)
    var timestamp = AudioTimeStamp()
    timestamp.mSampleTime = 10_000
    timestamp.mFlags = .sampleTimeValid
    let firstStatus = withUnsafePointer(to: &event) { pointer in
        unit.internalRenderBlock(&flags, &timestamp, 1, 0, output.mutableAudioBufferList, pointer, pull)
    }
    let expected: Float = 1 + (1.49 - 1) / 64
    let first = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(firstStatus == noErr && abs(first - expected) < 0.00001,
                "one-frame ramp readback was \(first), expected \(expected)")
    let applied = unit.parameterTargetForTesting(identifier: "height_gain") ?? .nan
    try require(abs(applied - 1.0) < 0.00001,
                "height ramp applied a future target before its time: \(applied)")
    guard let saved = unit.fullState,
          let savedValue = (saved["height_gain"] as? NSNumber)?.floatValue else {
        throw SmokeFailure.failure("cannot save interrupted height ramp")
    }
    try require(abs(savedValue - expected) < 0.00001, "saved mid-ramp value was \(savedValue)")
    guard let document = unit.exportPreset(named: "Mid-ramp") else {
        throw SmokeFailure.failure("cannot export mid-ramp preset")
    }

    timestamp.mSampleTime += 1
    parameter.eventSampleTime = AUEventSampleTime(timestamp.mSampleTime)
    parameter.rampDurationSampleFrames = 16
    parameter.value = 0.23
    event = AURenderEvent(parameter: parameter)
    let nextStatus = withUnsafePointer(to: &event) { pointer in
        unit.internalRenderBlock(&flags, &timestamp, 1, 0, output.mutableAudioBufferList, pointer, pull)
    }
    let secondExpected = expected + (0.23 - expected) / 16
    let second = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(nextStatus == noErr && abs(second - secondExpected) < 0.00001,
                "interrupted ramp started at \(second), expected \(secondExpected)")

    gain.value = 0.73
    timestamp.mSampleTime += 1
    let pointStatus = unit.internalRenderBlock(&flags, &timestamp, 1, 0, output.mutableAudioBufferList, nil, pull)
    let point = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(pointStatus == noErr && abs(point - 0.75) < 0.001,
                "mailbox point did not cancel the ramp: \(point)")
    timestamp.mSampleTime += 1
    _ = unit.internalRenderBlock(&flags, &timestamp, 1, 0, output.mutableAudioBufferList, nil, pull)
    let afterPoint = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(abs(afterPoint - 0.75) < 0.001, "ramp resumed after mailbox point: \(afterPoint)")
    unit.deallocateRenderResources()
    unit.fullState = saved
    let restored = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(abs(restored - expected) < 0.00001, "saved mid-ramp value restored as \(restored)")
    try require(unit.importPresetDocument(document), "cannot import mid-ramp preset")
    let imported = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
    try require(abs(imported - expected) < 0.00001, "mid-ramp preset restored as \(imported)")
    try unit.allocateRenderResources()
}

private func testUpmixerRampResourceRestart() throws {
    for changeLayout in [false, true] {
        let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
        let stereo = try stereoFormat(sampleRate: 48_000)
        let immersive = try format(sampleRate: 48_000, channels: 10, tag: kAudioChannelLayoutTag_Atmos_5_1_4)
        try unit.inputBusses[0].setFormat(stereo)
        try unit.outputBusses[0].setFormat(immersive)
        unit.maximumFramesToRender = 1
        try unit.allocateRenderResources()
        guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 1),
              let output = AVAudioPCMBuffer(pcmFormat: immersive, frameCapacity: 1),
              let gain = unit.parameterTree?.allParameters.first(where: { $0.identifier == "height_gain" }) else {
            throw SmokeFailure.failure("cannot prepare ramp resource restart test")
        }
        input.frameLength = 1
        output.frameLength = 1
        let pull: AURenderPullInputBlock = { _, _, _, _, destination in
            let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
            let target = UnsafeMutableAudioBufferListPointer(destination)
            for index in 0..<source.count {
                memcpy(target[index].mData, source[index].mData, Int(source[index].mDataByteSize))
                target[index].mDataByteSize = source[index].mDataByteSize
            }
            return noErr
        }
        var parameter = AUParameterEvent()
        parameter.eventSampleTime = 11_000
        parameter.eventType = .parameterRamp
        parameter.rampDurationSampleFrames = 64
        parameter.parameterAddress = gain.address
        parameter.value = 1.49
        var event = AURenderEvent(parameter: parameter)
        var flags = AudioUnitRenderActionFlags(rawValue: 0)
        var timestamp = AudioTimeStamp()
        timestamp.mSampleTime = 11_000
        timestamp.mFlags = .sampleTimeValid
        let status = withUnsafePointer(to: &event) { pointer in
            unit.internalRenderBlock(&flags, &timestamp, 1, 0, output.mutableAudioBufferList, pointer, pull)
        }
        try require(status == noErr, "ramp before resource restart failed: \(status)")
        let current: Float = 1 + (1.49 - 1) / 64
        unit.deallocateRenderResources()
        let newFormat: AVAudioFormat
        if changeLayout {
            newFormat = try format(sampleRate: 48_000, channels: 12, tag: kAudioChannelLayoutTag_Atmos_7_1_4)
            try unit.outputBusses[0].setFormat(newFormat)
        } else {
            newFormat = immersive
        }
        try unit.allocateRenderResources()
        defer { unit.deallocateRenderResources() }
        let restored = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
        try require(abs(restored - current) < 0.00001,
                    "resource restart changed ramp position: \(restored), layout changed=\(changeLayout)")
        guard let resumedOutput = AVAudioPCMBuffer(pcmFormat: newFormat, frameCapacity: 1) else {
            throw SmokeFailure.failure("cannot allocate restarted output")
        }
        resumedOutput.frameLength = 1
        timestamp.mSampleTime += 1
        let resumed = unit.internalRenderBlock(&flags, &timestamp, 1, 0, resumedOutput.mutableAudioBufferList, nil, pull)
        let after = unit.parameterValueForTesting(identifier: "height_gain") ?? .nan
        let wanted = changeLayout ? current : current + (1.49 - 1) / 64
        try require(resumed == noErr && abs(after - wanted) < 0.00001,
                    "unexpected ramp after resource restart: \(after), wanted \(wanted)")
    }
}

private func testUpmixerRampPartitionIndependence() throws {
    func render(blockSize: Int, mode: Int) throws -> [Float] {
        let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
        let stereo = try stereoFormat(sampleRate: 48_000)
        let immersive = try format(sampleRate: 48_000, channels: 10, tag: kAudioChannelLayoutTag_Atmos_5_1_4)
        try unit.inputBusses[0].setFormat(stereo)
        try unit.outputBusses[0].setFormat(immersive)
        unit.maximumFramesToRender = AUAudioFrameCount(blockSize)
        try unit.allocateRenderResources()
        defer { unit.deallocateRenderResources() }
        guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: AUAudioFrameCount(blockSize)),
              let output = AVAudioPCMBuffer(pcmFormat: immersive, frameCapacity: AUAudioFrameCount(blockSize)),
              let gain = unit.parameterTree?.allParameters.first(where: { $0.identifier == "height_gain" }) else {
            throw SmokeFailure.failure("cannot prepare height ramp partition test")
        }
        input.frameLength = AUAudioFrameCount(blockSize)
        output.frameLength = AUAudioFrameCount(blockSize)
        let pull: AURenderPullInputBlock = { _, _, _, _, destination in
            let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
            let target = UnsafeMutableAudioBufferListPointer(destination)
            for index in 0..<source.count {
                memcpy(target[index].mData, source[index].mData, Int(source[index].mDataByteSize))
                target[index].mDataByteSize = source[index].mDataByteSize
            }
            return noErr
        }
        var parameter = AUParameterEvent()
        parameter.eventSampleTime = 4096
        parameter.eventType = .parameterRamp
        parameter.rampDurationSampleFrames = 2048
        parameter.parameterAddress = gain.address
        parameter.value = 1.49
        var event = AURenderEvent(parameter: parameter)
        var interruptedParameter = AUParameterEvent()
        interruptedParameter.eventSampleTime = 4103
        interruptedParameter.eventType = .parameterRamp
        interruptedParameter.rampDurationSampleFrames = 1537
        interruptedParameter.parameterAddress = gain.address
        interruptedParameter.value = 0.37
        var interrupted = AURenderEvent(parameter: interruptedParameter)
        var flags = AudioUnitRenderActionFlags(rawValue: 0)
        var timestamp = AudioTimeStamp()
        timestamp.mFlags = .sampleTimeValid
        var samples: [Float] = []
        samples.reserveCapacity(8192 * 10)
        for position in stride(from: 0, to: 8192, by: blockSize) {
            timestamp.mSampleTime = Float64(position)
            for frame in 0..<blockSize {
                for channel in 0..<2 {
                    var bits = UInt32(position + frame) &* 1_664_525 &+ UInt32(channel) &* 1_013_904_223 &+ 12_345
                    bits ^= bits >> 13
                    bits = bits &* 2_246_822_519
                    input.floatChannelData![channel][frame] = (Float(bits >> 8) / 16_777_216 - 0.5) * 0.2
                }
            }
            let first = mode > 0 && position <= 4096 && 4096 < position + blockSize
            let second = mode == 2 && position <= 4103 && 4103 < position + blockSize
            let status = withUnsafeMutablePointer(to: &interrupted) { secondPointer in
                event.parameter.next = first && second ? secondPointer : nil
                return withUnsafePointer(to: &event) { firstPointer in
                    let events = first ? firstPointer : (second ? UnsafePointer(secondPointer) : nil)
                    return unit.internalRenderBlock(&flags, &timestamp, AUAudioFrameCount(blockSize), 0,
                        output.mutableAudioBufferList, events, pull)
                }
            }
            try require(status == noErr, "height ramp partition render failed: \(status)")
            for frame in 0..<blockSize {
                for channel in 0..<10 { samples.append(output.floatChannelData![channel][frame]) }
            }
        }
        return samples
    }
    for mode in [0, 1, 2] {
        let single = try render(blockSize: 1, mode: mode)
        let wide = try render(blockSize: 64, mode: mode)
        try require(single == wide, "height ramp output changed with callback partition, mode=\(mode)")
    }
}

private func testUpmixerSimultaneousRampTiming() throws {
    func trace(withHeight: Bool) throws -> (values: [Float], bed: [Float]) {
        let unit = try UpmixerAudioUnit(componentDescription: componentDescription("SOUp"))
        let stereo = try stereoFormat(sampleRate: 48_000)
        let immersive = try format(sampleRate: 48_000, channels: 10, tag: kAudioChannelLayoutTag_Atmos_5_1_4)
        try unit.inputBusses[0].setFormat(stereo)
        try unit.outputBusses[0].setFormat(immersive)
        unit.maximumFramesToRender = 32
        try unit.allocateRenderResources()
        defer { unit.deallocateRenderResources() }
        guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 32),
              let output = AVAudioPCMBuffer(pcmFormat: immersive, frameCapacity: 32),
              let height = unit.parameterTree?.allParameters.first(where: { $0.identifier == "height_gain" }),
              let width = unit.parameterTree?.allParameters.first(where: { $0.identifier == "stereo_width" }) else {
            throw SmokeFailure.failure("cannot prepare simultaneous ramps")
        }
        input.frameLength = 32
        output.frameLength = 32
        func fillInput(_ position: Int) {
            for frame in 0..<32 {
                for channel in 0..<2 {
                    var bits = UInt32(position + frame) &* 1_664_525 &+ UInt32(channel) &* 1_013_904_223 &+ 12_345
                    bits ^= bits >> 13
                    bits = bits &* 2_246_822_519
                    input.floatChannelData![channel][frame] = (Float(bits >> 8) / 16_777_216 - 0.5) * 0.2
                }
            }
        }
        var bed: [Float] = []
        bed.reserveCapacity(160 * 32 * 6)
        func captureBed() {
            for frame in 0..<32 {
                for channel in 0..<6 { bed.append(output.floatChannelData![channel][frame]) }
            }
        }
        let pull: AURenderPullInputBlock = { _, _, _, _, destination in
            let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
            let target = UnsafeMutableAudioBufferListPointer(destination)
            for index in 0..<source.count {
                memcpy(target[index].mData, source[index].mData, Int(source[index].mDataByteSize))
                target[index].mDataByteSize = source[index].mDataByteSize
            }
            return noErr
        }
        var widthParameter = AUParameterEvent()
        widthParameter.eventSampleTime = 12_000
        widthParameter.eventType = .parameterRamp
        widthParameter.rampDurationSampleFrames = 96
        widthParameter.parameterAddress = width.address
        widthParameter.value = 0.77
        var widthEvent = AURenderEvent(parameter: widthParameter)
        var heightParameter = AUParameterEvent()
        heightParameter.eventSampleTime = 12_000
        heightParameter.eventType = .parameterRamp
        heightParameter.rampDurationSampleFrames = 19
        heightParameter.parameterAddress = height.address
        heightParameter.value = 1.49
        var heightEvent = AURenderEvent(parameter: heightParameter)
        var flags = AudioUnitRenderActionFlags(rawValue: 0)
        var timestamp = AudioTimeStamp()
        timestamp.mSampleTime = 12_000
        timestamp.mFlags = .sampleTimeValid
        fillInput(0)
        let status = withUnsafeMutablePointer(to: &heightEvent) { heightPointer in
            if withHeight { widthEvent.parameter.next = heightPointer }
            return withUnsafePointer(to: &widthEvent) { pointer in
                unit.internalRenderBlock(&flags, &timestamp, 32, 0, output.mutableAudioBufferList, pointer, pull)
            }
        }
        try require(status == noErr, "simultaneous ramp start failed: \(status)")
        captureBed()
        var values = [unit.parameterValueForTesting(identifier: "stereo_width") ?? .nan]
        for block in 1..<160 {
            timestamp.mSampleTime += 32
            fillInput(block * 32)
            let next = unit.internalRenderBlock(&flags, &timestamp, 32, 0, output.mutableAudioBufferList, nil, pull)
            try require(next == noErr, "simultaneous ramp continuation failed: \(next)")
            captureBed()
            values.append(unit.parameterValueForTesting(identifier: "stereo_width") ?? .nan)
        }
        return (values, bed)
    }
    let reference = try trace(withHeight: false)
    let mixed = try trace(withHeight: true)
    try require(reference.values == mixed.values, "precise ramp changed another ramp's timing")
    try require(reference.bed == mixed.bed && reference.bed.contains(where: { $0 != 0 }),
                "precise ramp changed the non-silent bed during another ramp")
}

private func testConcurrentRenderAndControlAccess() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(stereo)
    unit.maximumFramesToRender = 64
    try unit.allocateRenderResources()

    guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 64),
          let output = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 64) else {
        unit.deallocateRenderResources()
        throw SmokeFailure.failure("cannot allocate concurrency stress buffers")
    }
    input.frameLength = 64
    output.frameLength = 64
    let pullInput: AURenderPullInputBlock = { _, _, _, _, destination in
        let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
        let target = UnsafeMutableAudioBufferListPointer(destination)
        guard source.count == target.count else { return kAudioUnitErr_InvalidPropertyValue }
        for index in 0..<source.count {
            guard let src = source[index].mData, let dst = target[index].mData else {
                return kAudioUnitErr_NoConnection
            }
            memcpy(dst, src, Int(source[index].mDataByteSize))
            target[index].mDataByteSize = source[index].mDataByteSize
        }
        return noErr
    }
    let block = unit.internalRenderBlock
    let completion = DispatchSemaphore(value: 0)
    let failures = StressFailureBox()
    Thread.detachNewThread {
        var flags = AudioUnitRenderActionFlags(rawValue: 0)
        var timestamp = AudioTimeStamp()
        for index in 0..<100_000 {
            timestamp.mSampleTime = Float64(index * 64)
            let status = block(
                &flags, &timestamp, 64, 0, output.mutableAudioBufferList, nil, pullInput
            )
            if status != noErr
                && status != kAudioUnitErr_CannotDoInCurrentContext
                && status != kAudioUnitErr_Uninitialized {
                failures.record("concurrent render returned \(status)")
                break
            }
        }
        completion.signal()
    }

    guard let gain = unit.parameterTree?.allParameters.first(where: {
        $0.identifier == "gain_db"
    }) else {
        unit.deallocateRenderResources()
        throw SmokeFailure.failure("Gain concurrency stress is missing gain_db")
    }
    for index in 0..<2_000 {
        gain.value = AUValue((index % 25) - 12)
        if index % 50 == 0, let state = unit.fullState {
            unit.fullState = state
        }
    }
    // Exercise complete handle/scratch publication while the saved render
    // block is actively attempting blocks. Reconfiguration allocations and
    // handle retirement occur on this control thread; render is nonblocking.
    for index in 0..<20 {
        let negotiated = try stereoFormat(sampleRate: index.isMultiple(of: 2) ? 96_000 : 48_000)
        unit.deallocateRenderResources()
        try unit.inputBusses[0].setFormat(negotiated)
        try unit.outputBusses[0].setFormat(negotiated)
        try unit.allocateRenderResources()
    }
    let waitResult = completion.wait(timeout: .now() + 30)
    unit.deallocateRenderResources()
    try require(waitResult == .success, "concurrent render/control stress timed out")
    if let message = failures.take() {
        throw SmokeFailure.failure(message)
    }
    let (_, recovered) = try render(
        unit,
        inputFormat: stereo,
        outputFormat: stereo,
        frames: 64
    )
    try assertFiniteOverwrittenAndAudible(recovered, caseName: "concurrency stress recovery")
}

private func testRealtimeRenderAllocationEvidence() throws {
    let unit = try GainAudioUnit(componentDescription: componentDescription("SOGN"))
    let stereo = try stereoFormat(sampleRate: 48_000)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(stereo)
    unit.maximumFramesToRender = 64
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }
    guard let input = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 64),
          let output = AVAudioPCMBuffer(pcmFormat: stereo, frameCapacity: 64) else {
        throw SmokeFailure.failure("cannot allocate realtime-allocation test buffers")
    }
    input.frameLength = 64
    output.frameLength = 64
    let inputBufferList = input.mutableAudioBufferList
    let outputBufferList = output.mutableAudioBufferList
    let pullInput: AURenderPullInputBlock = { _, _, _, _, destination in
        let source = UnsafeMutableAudioBufferListPointer(inputBufferList)
        let target = UnsafeMutableAudioBufferListPointer(destination)
        guard source.count == target.count else { return kAudioUnitErr_InvalidPropertyValue }
        for index in 0..<source.count {
            guard let src = source[index].mData, let dst = target[index].mData else {
                return kAudioUnitErr_NoConnection
            }
            memcpy(dst, src, Int(source[index].mDataByteSize))
            target[index].mDataByteSize = source[index].mDataByteSize
        }
        return noErr
    }
    let block = unit.internalRenderBlock
    var flags = AudioUnitRenderActionFlags(rawValue: 0)
    var timestamp = AudioTimeStamp()
    func run(_ count: Int) throws {
        for index in 0..<count {
            timestamp.mSampleTime = Float64(index * 64)
            let status = block(
                &flags, &timestamp, 64, 0, outputBufferList, nil, pullInput
            )
            try require(status == noErr, "allocation-evidence render returned \(status)")
        }
    }
    try run(256)
    var firstFailure: OSStatus = noErr
    sotfRenderCounterArm()
    for index in 0..<20_000 {
        timestamp.mSampleTime = Float64(index * 64)
        let status = block(
            &flags, &timestamp, 64, 0, outputBufferList, nil, pullInput
        )
        if status != noErr {
            firstFailure = status
            break
        }
    }
    var allocations: UInt64 = 0
    var frees: UInt64 = 0
    sotfRenderCounterDisarm(&allocations, &frees)
    try require(firstFailure == noErr, "allocation-evidence render returned \(firstFailure)")
    try require(
        allocations == 0 && frees == 0,
        "realtime render called the allocator: allocations=\(allocations), frees=\(frees)"
    )
}

private func testAllocatorInterpositionSelfTest() throws {
    var allocations: UInt64 = 0
    var frees: UInt64 = 0
    sotfMallocCounterSelfTest(&allocations, &frees)
    try require(
        allocations >= 6 && frees >= 5,
        "allocator interposition missed covered APIs: allocations=\(allocations), frees=\(frees)"
    )
}

private func testSpectrumAnalyzerNativeRender() throws {
    let sampleRate = 48_000.0
    let format = try stereoFormat(sampleRate: sampleRate)
    let unit = try SpectrumAnalyzerAudioUnit(
        componentDescription: componentDescription("SOSa")
    )
    let (input, output) = try render(
        unit,
        inputFormat: format,
        outputFormat: format,
        frames: 4096
    )
    try require(unit.hasRustPlugin, "Spectrum Analyzer Rust plugin did not instantiate")
    guard let inputChannels = input.floatChannelData,
          let outputChannels = output.floatChannelData else {
        throw SmokeFailure.failure("Spectrum Analyzer native buffers are not Float32 planar")
    }
    for channel in 0..<2 {
        for frame in 0..<4096 {
            try require(
                inputChannels[channel][frame] == outputChannels[channel][frame],
                "Spectrum Analyzer native render is not bit-transparent"
            )
        }
    }
}

private func testLinearPhaseNegotiatedAdapterLatency() throws {
    let unit = try LinearPhaseEQAudioUnit(
        componentDescription: componentDescription("SOLP")
    )
    let stereo = try stereoFormat(sampleRate: 96_000)
    try unit.inputBusses[0].setFormat(stereo)
    try unit.outputBusses[0].setFormat(stereo)
    let observed = Atomic<Bool>(false)
    let observation = unit.observe(\.latency, options: [.new]) { _, change in
        if change.newValue != nil {
            observed.store(true, ordering: .releasing)
        }
    }
    defer { observation.invalidate() }

    unit.maximumFramesToRender = 128
    try unit.allocateRenderResources()
    let small = unit.latency
    unit.deallocateRenderResources()
    try require(
        observed.load(ordering: .acquiring),
        "LinearPhaseEQ initial negotiated latency was not published through KVO"
    )
    observed.store(false, ordering: .releasing)

    // Reallocate with the same format/rate/configuration and only a different
    // callback maximum. This specifically verifies that the negotiated bound
    // is part of the Rust-handle recreation identity.
    unit.maximumFramesToRender = 257
    try unit.allocateRenderResources()
    defer { unit.deallocateRenderResources() }
    let large = unit.latency
    try require(
        observed.load(ordering: .acquiring),
        "LinearPhaseEQ callback-bound latency replacement was not published through KVO"
    )

    // The small callback maximum equals LinearPhaseEQ's 128-frame scheduling
    // horizon; the large one dominates it, so the latency delta is exact.
    let expectedDelta = 2.0 * Double(257 - 128) / 96_000.0
    try require(
        abs((large - small) - expectedDelta) < 1.0e-9,
        "LinearPhaseEQ adapter latency did not track negotiated callback quantum: small=\(small), large=\(large), expectedDelta=\(expectedDelta)"
    )
}

private func runSmokeTests() throws {
    if ProcessInfo.processInfo.environment["SOTF_AU_ALLOCATION_ONLY"] == "1" {
        try testAllocatorInterpositionSelfTest()
        try testUpmixerNativeRenderAllocation()
        print("Native AU allocation tests passed")
        return
    }
    print("Running constructor-failure tests")
    try testRepeatedConstructorFailureIsDeterministic()
    print("Running wrapper inventory tests")
    try testEveryPublishedWrapperConstructs()
    print("Running default topology tests")
    try testDefaultChannelChangingTopologies()
    print("Running fixed topology rejection tests")
    try testFixedTopologyRejectionAndRecovery()
    print("Running variable topology rejection tests")
    try testVariableToStereoCapabilityRejectionAndRecovery()
    print("Running mono-to-stereo tests")
    try testMonoToStereoConstructionAndRender()
    print("Running downmix layout tests")
    try testDownmixLayoutMatrix()
    print("Running upmixer output layout recreation tests")
    try testUpmixerOutputLayoutRecreation()
    print("Running upmixer reinitializing state restore tests")
    try testUpmixerReinitializingStateRestore()
    try testUpmixerStructuralParameterMetadata()
    print("Running downmix format-change tests")
    try testDownmixFormatChangeAndUnsupportedLayout()
    print("Running named-layout refusal preservation tests")
    try testTaggedAtmosOrderRefusalPreservesWrapperState()
    print("Running spectrum tests")
    try testSpectrumAnalyzerNativeRender()
    print("Running LinearPhaseEQ negotiated adapter latency tests")
    try testLinearPhaseNegotiatedAdapterLatency()
    print("Running state migration tests")
    try testParameterStateSurvivesFormatRecreation()
    try testMigrationFailureIsTransactional()
    try testTopologyMigrationUsesPortableParameters()
    try testFullStatePartialFailureIsTransactional()
    try testFullStateRestoresHostParameterMap()
    try testResourcePreparationFailureRollback()
    try testRetainedRenderBlockOwnsInputStorage()
    print("Running strict format negotiation tests")
    try testStrictFormatNegotiationAndRecovery()
    print("Running render bounds and output ownership tests")
    try testRenderBoundsAndOutputOwnership()
    print("Running 32-channel capability tests")
    try testAuthoritativeChannelCapabilitySemantics()
    try testThirtyTwoChannelCapsAndRecovery()
    print("Running parameter mailbox ordering/coalescing tests")
    try testParameterQueueContentionOrderAndOverflow()
    print("Running SpeechDenoiser pre-allocation negotiation tests")
    try testSpeechDenoiserNegotiationBeforeAllocation()
    print("Running parameter render-event point/ramp tests")
    try testAbsoluteEventTimelineConversion()
    try testParameterRenderEventPointAndRampOffsets()
    try testParameterRampPersistsAcrossBlocks()
    try testUpmixerStatePreservesUnsteppedValues()
    try testUpmixerHeightRampPrecision()
    try testUpmixerPreciseRampInterruption()
    try testUpmixerRampResourceRestart()
    try testUpmixerRampPartitionIndependence()
    try testUpmixerSimultaneousRampTiming()
    print("Running concurrent render/control stress")
    try testQueuedControlHasBoundedRenderFairness()
    try testConcurrentRenderAndControlAccess()
    print("Running realtime allocation evidence")
    try testAllocatorInterpositionSelfTest()
    try testRealtimeRenderAllocationEvidence()
    try testUpmixerNativeRenderAllocation()
    print("Native AU smoke tests passed")
}

do {
    try runSmokeTests()
} catch {
    fputs("Native AU smoke tests failed: \(error)\n", stderr)
    exit(EXIT_FAILURE)
}
