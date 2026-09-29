// ============================================================================
// Symphonia-backed substream decoders (AAC-LC, FLAC)
// ============================================================================
//
// Pure-Rust IAMF substream decoding via Symphonia's AAC and FLAC decoders.
// IAMF audio frames carry raw codec packets (no ADTS/container framing);
// codec setup comes from the codec_config `decoder_config` bytes:
// AudioSpecificConfig for `mp4a`, the STREAMINFO metadata block for `fLaC`.

use super::SubstreamDecoder;
use crate::error::{IamfError, IamfResult};

use symphonia_bundle_flac::FlacDecoder as SymFlacDecoder;
use symphonia_codec_aac::AacDecoder as SymAacDecoder;
use symphonia_core::audio::{Audio, AudioBuffer, GenericAudioBufferRef};
use symphonia_core::codecs::audio::well_known::{CODEC_ID_AAC, CODEC_ID_FLAC};
use symphonia_core::codecs::audio::{
    AudioCodecId, AudioCodecParameters, AudioDecoder, AudioDecoderOptions,
};
use symphonia_core::packet::PacketRef;
use symphonia_core::units::{Duration, Timestamp};

/// Shared packet pump: wrap one raw IAMF frame payload as a Symphonia
/// packet, decode, and return planar-to-interleaved f32 samples in
/// [-1.0, 1.0].
fn decode_packet<D: AudioDecoder>(
    decoder: &mut D,
    payload: &[u8],
    what: &str,
) -> IamfResult<Vec<f32>> {
    let packet = PacketRef::new(0, Timestamp::default(), Duration::default(), payload);
    let decoded = decoder
        .decode_ref(&packet)
        .map_err(|e| IamfError::CodecError(format!("{what} decode failed: {e}")))?;

    let frames = decoded.frames();
    let nch = decoded.spec().channels().count();
    let mut out = vec![0.0f32; frames * nch];
    match decoded {
        GenericAudioBufferRef::F32(buf) => interleave(buf, &mut out, frames, nch, 1.0),
        // Symphonia's FLAC decoder left-justifies samples to 32 bits.
        GenericAudioBufferRef::S32(buf) => {
            interleave_i32(buf, &mut out, frames, nch, 1.0 / 2_147_483_648.0)
        }
        _ => {
            return Err(IamfError::CodecError(format!(
                "{what} produced an unexpected sample format"
            )));
        }
    }
    Ok(out)
}

fn interleave(buf: &AudioBuffer<f32>, out: &mut [f32], frames: usize, nch: usize, scale: f32) {
    for c in 0..nch {
        if let Some(plane) = buf.plane(c) {
            for (f, sample) in plane.iter().enumerate().take(frames) {
                out[f * nch + c] = *sample * scale;
            }
        }
    }
}

fn interleave_i32(buf: &AudioBuffer<i32>, out: &mut [f32], frames: usize, nch: usize, scale: f32) {
    for c in 0..nch {
        if let Some(plane) = buf.plane(c) {
            for (f, sample) in plane.iter().enumerate().take(frames) {
                out[f * nch + c] = (*sample as f32) * scale;
            }
        }
    }
}

fn base_params(
    codec: AudioCodecId,
    sample_rate: u32,
    decoder_config: &[u8],
) -> AudioCodecParameters {
    let mut params = AudioCodecParameters::new();
    params.for_codec(codec);
    params.with_sample_rate(sample_rate);
    if !decoder_config.is_empty() {
        params.with_extra_data(decoder_config.to_vec().into_boxed_slice());
    }
    params
}

/// AAC-LC substream decoder (IAMF `mp4a`).
///
/// `decoder_config` must carry the AudioSpecificConfig. Symphonia's AAC
/// decoder supports LC stereo/mono at 1024 samples per frame; anything
/// else is rejected at construction.
pub struct AacSubstreamDecoder {
    inner: SymAacDecoder,
    channels: usize,
}

impl AacSubstreamDecoder {
    pub fn new(channels: usize, sample_rate: u32, decoder_config: &[u8]) -> IamfResult<Self> {
        if decoder_config.is_empty() {
            return Err(IamfError::CodecError(
                "AAC substream needs an AudioSpecificConfig decoder_config".into(),
            ));
        }
        let params = base_params(CODEC_ID_AAC, sample_rate, decoder_config);
        let inner = SymAacDecoder::try_new(&params, &AudioDecoderOptions::default())
            .map_err(|e| IamfError::CodecError(format!("AAC setup failed: {e}")))?;
        Ok(Self { inner, channels })
    }
}

impl SubstreamDecoder for AacSubstreamDecoder {
    fn decode_frame(&mut self, payload: &[u8]) -> IamfResult<Vec<f32>> {
        decode_packet(&mut self.inner, payload, "AAC")
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// FLAC substream decoder (IAMF `fLaC`).
///
/// `decoder_config` must carry the 34-byte STREAMINFO metadata block.
pub struct FlacSubstreamDecoder {
    inner: SymFlacDecoder,
    channels: usize,
}

impl FlacSubstreamDecoder {
    pub fn new(channels: usize, sample_rate: u32, decoder_config: &[u8]) -> IamfResult<Self> {
        if decoder_config.len() < 34 {
            return Err(IamfError::CodecError(format!(
                "FLAC substream needs a STREAMINFO decoder_config, got {} bytes",
                decoder_config.len()
            )));
        }
        let params = base_params(CODEC_ID_FLAC, sample_rate, decoder_config);
        let inner = SymFlacDecoder::try_new(&params, &AudioDecoderOptions::default())
            .map_err(|e| IamfError::CodecError(format!("FLAC setup failed: {e}")))?;
        let channels = inner
            .codec_params()
            .channels
            .as_ref()
            .map(|c| c.count())
            .unwrap_or(channels);
        Ok(Self { inner, channels })
    }
}

impl SubstreamDecoder for FlacSubstreamDecoder {
    fn decode_frame(&mut self, payload: &[u8]) -> IamfResult<Vec<f32>> {
        decode_packet(&mut self.inner, payload, "FLAC")
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// STREAMINFO for 48 kHz mono 16-bit, max block 32 (from `flac`
/// encoding a 440 Hz sine).
#[cfg(test)]
pub(crate) const TEST_STREAMINFO: &str =
    "0020002000001900001f0bb800f000005dc01964e7953bdf1b99cddc8a6487e0a0d0";
/// First raw FLAC frame (32 samples).
#[cfg(test)]
pub(crate) const TEST_FLAC_FRAME0: &str =
    "fff86a08001f7a16000000eb01d6007790d46bd1c47591c4d72ca6d6";
/// AudioSpecificConfig: AAC-LC, 48 kHz, stereo.
#[cfg(test)]
pub(crate) const TEST_ASC: &str = "1190";
/// First raw AAC frame (stereo priming frame, 1024 samples/channel).
#[cfg(test)]
pub(crate) const TEST_AAC_FRAME0: &str = "de02004c61766336302e33312e3130320042501fffffe00a4b4a3b4c41d13eb278d4f5f56d54b91722392249f1700c2bb9d2d5059eb28c5bf01bb6157e7a28c7845a1b4b037c18181819103031b060606448911b366c181912206366cd9b448914b29b8a28a28a28a2a41be9bc3a27d64f1a9ebeadaa9722e44724491700000000000070";

#[cfg(test)]
mod tests {
    use super::*;

    fn h(hex: &str) -> Vec<u8> {
        assert!(hex.len().is_multiple_of(2));
        hex.as_bytes()
            .chunks(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn flac_decodes_first_frame_bit_exact() {
        let mut dec = FlacSubstreamDecoder::new(1, 48000, &h(TEST_STREAMINFO)).expect("FLAC setup");
        assert_eq!(dec.channels(), 1);
        let pcm = dec.decode_frame(&h(TEST_FLAC_FRAME0)).expect("decode");
        // Lossless 32-sample sine start, verified against ffmpeg decode.
        let expected = [
            0.0f32,
            0.007171631,
            0.014343262,
            0.021484375,
            0.028533936,
            0.035461426,
            0.04232788,
            0.04901123,
            0.055541992,
            0.061920166,
            0.0680542,
            0.07397461,
            0.07965088,
            0.08505249,
            0.09020996,
            0.09503174,
            0.09951782,
            0.10372925,
            0.10757446,
            0.11105347,
            0.11416626,
            0.116882324,
            0.119262695,
            0.12121582,
            0.1227417,
            0.12390137,
            0.12463379,
            0.12496948,
            0.12487793,
            0.12435913,
            0.1234436,
            0.12210083,
        ];
        assert_eq!(pcm.len(), expected.len());
        for (got, want) in pcm.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-9, "got {got}, want {want}");
        }
    }

    #[test]
    fn flac_rejects_short_streaminfo() {
        match FlacSubstreamDecoder::new(1, 48000, &[0u8; 10]) {
            Err(IamfError::CodecError(_)) => {}
            _ => panic!("short STREAMINFO must be a CodecError"),
        }
    }

    #[test]
    fn flac_rejects_garbage_frame() {
        let mut dec = FlacSubstreamDecoder::new(1, 48000, &h(TEST_STREAMINFO)).expect("FLAC setup");
        let err = dec.decode_frame(&[0u8; 28]).unwrap_err();
        assert!(matches!(err, IamfError::CodecError(_)));
    }

    /// Three consecutive raw AAC frames (stereo 440 Hz sine, 64 kbit/s):
    /// priming frame, onset, then steady sine.
    const AAC_FRAMES: [&str; 3] = [
        "de02004c61766336302e33312e3130320042501fffffe00a4b4a3b4c41d13eb278d4f5f56d54b91722392249f1700c2bb9d2d5059eb28c5bf01bb6157e7a28c7845a1b4b037c18181819103031b060606448911b366c181912206366cd9b448914b29b8a28a28a28a2a41be9bc3a27d64f1a9ebeadaa9722e44724491700000000000070",
        "214a6cfe1f87e1f84c592bb2c97564bab29d37fbf9d5b6f5739ffe3f7f3ad71abd7edffc7efe75ae357af7ffe9f8f3ad6b5ab1b53b36eb9e798091b4a4f4b5ace54cfcee7fd092d6d4060dd03e942be940fa7d35980fa7d3e9401f4fa7d3581f4fa7d35d06007d3e9f4d666c00451a021b7a64120a279dd75a6239aac4421899f2899f2899f2f9444c1f2f9718983e5f2e24c1f2f97ca288801f2f97cb8c51000950e584fc079c59c0909090909090909090909090939303b60ef83be0ef8dfefe756dad5ce7ff8fdfceb415ffc7efe75a0e3ffa7e3ceb400000000000000038",
        "2178cffffff0009667127502427dbfbff3c5ebae27522e5cd48e2b92d57ed4036a719a0b7163cf1f0c719a666427ae491dc0d99a666477dfde13fc7c363dfde11f1f1987bfbc27c9f0da5d44d9fc3b268c04c913c1d4abaf3b2586faaf113edfdff9e2f5d713a9172e6a4715c96ab400000000000070",
    ];

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn aac_decodes_three_frames() {
        let mut dec = AacSubstreamDecoder::new(2, 48000, &h(TEST_ASC)).expect("AAC setup");
        assert_eq!(dec.channels(), 2);
        let frames: Vec<Vec<f32>> = AAC_FRAMES
            .iter()
            .map(|f| dec.decode_frame(&h(f)).expect("decode"))
            .collect();
        for pcm in &frames {
            // Stereo, 1024 samples per channel.
            assert_eq!(pcm.len(), 2048);
        }
        // Frame 0 is encoder priming (near silence); frames 1-2 carry the
        // sine onset. Levels verified against ffmpeg's AAC decoder.
        assert!(rms(&frames[0]) < 0.01);
        assert!((rms(&frames[1]) - 0.06246152).abs() < 1e-6);
        assert!((rms(&frames[2]) - 0.05876831).abs() < 1e-6);
        // Deterministic decoder output: exact onset samples.
        let want = [0.004091443f32, 0.004063033, 0.007198739, 0.007190457];
        for (got, w) in frames[1][..4].iter().zip(want.iter()) {
            assert!((got - w).abs() < 1e-6, "got {got}, want {w}");
        }
    }

    #[test]
    fn aac_rejects_missing_asc() {
        match AacSubstreamDecoder::new(2, 48000, &[]) {
            Err(IamfError::CodecError(_)) => {}
            _ => panic!("missing ASC must be a CodecError"),
        }
    }

    #[test]
    fn aac_rejects_garbage_frame() {
        let mut dec = AacSubstreamDecoder::new(2, 48000, &h(TEST_ASC)).expect("AAC setup");
        let err = dec.decode_frame(&[0u8; 132]).unwrap_err();
        assert!(matches!(err, IamfError::CodecError(_)));
    }
}
