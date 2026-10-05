// ============================================================================
// Corpus conformance: reference IAMF bitstreams from AOMediaCodec/iamf-tools
// ============================================================================
//
// `data/` holds the small (<5 kB) reference files from
// `iamf/cli/testdata/iamf` (see README.upstream.md). These tests prove the
// whole pipeline — OBU framing, descriptors, substream decode, render, mix
// — against independently produced bitstreams, not synthetic fixtures.

use sotf_iamf::IamfDecoder;
use sotf_iamf::error::IamfError;
use sotf_iamf::obu::parser::parse_descriptors;
use sotf_iamf::types::{AudioElementType, CodecId, IamfChannelLayout};
use std::io::Cursor;

fn data(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../../symphonia-add-ons/symphonia-iamf-core/tests/data/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|_| panic!("missing corpus file {path}"))
}

fn decode_all(bytes: &[u8], layout: Option<&str>, unit_cap: usize) -> (Vec<f32>, u64, usize) {
    let mut dec = IamfDecoder::open(Cursor::new(bytes)).expect("open corpus stream");
    if let Some(layout) = layout {
        dec.set_output_layout(layout).expect("switch layout");
    }
    let ch = dec.spec().output_channels as usize;
    let mut unit = vec![0.0f32; unit_cap * ch];
    let mut pcm = Vec::new();
    let mut total = 0u64;
    loop {
        let n = dec.decode_next(&mut unit).expect("decode corpus unit");
        pcm.extend_from_slice(&unit[..n * ch]);
        total += n as u64;
        if dec.is_eof() {
            break;
        }
    }
    (pcm, total, ch)
}

fn rms(v: &[f32]) -> f32 {
    (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
}

#[test]
fn flac_stereo_reference_decodes() {
    let bytes = data("noise_1024samp_stereo_flac.iamf");

    // Descriptors: stereo FLAC element, one stereo loudness layout.
    let (d, _) = parse_descriptors(&bytes).expect("parse FLAC descriptors");
    assert_eq!(d.codec_configs.len(), 1);
    let cc = &d.codec_configs[0];
    assert_eq!(cc.codec_id, CodecId::Flac);
    assert_eq!((cc.sample_rate, cc.bit_depth), (48000, 16));
    // The STREAMINFO metadata-block header is stripped for the decoder.
    assert_eq!(cc.decoder_config.len(), 34);
    assert_eq!(d.audio_elements.len(), 1);
    let el = &d.audio_elements[0];
    assert_eq!(el.element_type, AudioElementType::Channel);
    assert_eq!(d.mix_presentations.len(), 1);
    let layouts = &d.mix_presentations[0].sub_mixes[0].layouts;
    assert_eq!(layouts.len(), 1);
    assert_eq!(layouts[0].layout, IamfChannelLayout::Stereo);

    // Full decode: one unit, no trim metadata, so every packet sample.
    let (pcm, total, ch) = decode_all(&bytes, None, 4608);
    assert_eq!(ch, 2);
    assert_eq!(total, 4608);
    assert_eq!(pcm.len(), 4608 * 2);
    let left: Vec<f32> = pcm.iter().step_by(2).copied().collect();
    let right: Vec<f32> = pcm.iter().skip(1).step_by(2).copied().collect();
    // White noise, different per channel: energy present, channels differ.
    for x in [&left, &right] {
        let r = rms(x);
        assert!(r > 0.05 && r < 0.9, "rms {r}");
        assert!(x.iter().all(|s| s.abs() <= 1.0));
    }
    let diff = left
        .iter()
        .zip(right.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(diff > 0.1, "channels must differ");

    // Deterministic across seek + re-decode.
    let (pcm2, total2) = {
        let mut dec = IamfDecoder::open(Cursor::new(&bytes)).expect("reopen");
        let mut unit = vec![0.0f32; 4608 * 2];
        let n = dec.decode_next(&mut unit).expect("first decode");
        dec.seek(0).expect("rewind");
        let mut unit2 = vec![0.0f32; 4608 * 2];
        let m = dec.decode_next(&mut unit2).expect("second decode");
        assert_eq!(n, m);
        (unit2[..m * 2].to_vec(), m as u64)
    };
    assert_eq!(total, total2);
    assert_eq!(pcm, pcm2);
}

#[test]
fn pcm_51_reference_decodes() {
    let bytes = data("tones_256samp_5p1_pcm.iamf");

    // Descriptors: single-layer 5.1 over 4 substreams, stereo mix layout,
    // LPCM with no declared frame size.
    let (d, _) = parse_descriptors(&bytes).expect("parse PCM descriptors");
    let cc = &d.codec_configs[0];
    assert_eq!(cc.codec_id, CodecId::Lpcm);
    assert_eq!(cc.num_samples_per_frame, 0);
    assert_eq!((cc.sample_rate, cc.bit_depth), (48000, 16));
    let el = &d.audio_elements[0];
    assert_eq!(el.substream_ids.len(), 4);
    match &el.element_config {
        sotf_iamf::types::ElementConfig::Channel(cfg) => {
            assert_eq!(cfg.layers.len(), 1);
            assert_eq!(
                cfg.layers[0].loudspeaker_layout,
                IamfChannelLayout::Layout5_1
            );
            assert_eq!(cfg.layers[0].substream_count, 4);
        }
        other => panic!("expected channel element, got {other:?}"),
    }

    // Default (stereo) render: 256 frames of non-silent signal.
    let (stereo, total, ch) = decode_all(&bytes, None, 512);
    assert_eq!((ch, total), (2, 256));
    assert!(rms(&stereo) > 0.1);
    assert!(stereo.iter().all(|s| s.abs() <= 1.0));

    // Explicit 5.1 render: six distinct non-silent channels.
    let (surround, total51, ch51) = decode_all(&bytes, Some("5.1"), 512);
    assert_eq!((ch51, total51), (6, 256));
    let channels: Vec<Vec<f32>> = (0..6)
        .map(|c| surround.iter().skip(c).step_by(6).copied().collect())
        .collect();
    for (i, x) in channels.iter().enumerate() {
        let r = rms(x);
        assert!(r > 0.1 && r <= 1.0, "ch{i} rms {r}");
    }
    for (i, a) in channels.iter().enumerate() {
        for (j, b) in channels.iter().enumerate().skip(i + 1) {
            let diff = a
                .iter()
                .zip(b.iter())
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max);
            assert!(diff > 0.01, "channels {i} and {j} must differ");
        }
    }
}

#[test]
fn opus_reference_files_parse_but_need_engine_decoder() {
    // Opus has no pure-Rust decoder: descriptors must parse (proving OBU
    // coverage incl. the ambisonics element), while opening a player
    // reports UnsupportedCodec instead of misdecoding.
    for name in [
        "noise_1024samp_5p1_opus.iamf",
        "noise_3s_stereo_opus.iamf",
        "tones_100ms_3OA_stereo_opus.iamf",
    ] {
        let bytes = data(name);
        let (d, _) = parse_descriptors(&bytes).expect("parse Opus descriptors");
        assert!(!d.audio_elements.is_empty(), "{name}");
        assert!(!d.mix_presentations.is_empty(), "{name}");
        match IamfDecoder::open(Cursor::new(&bytes)) {
            Err(IamfError::UnsupportedCodec(_)) => {}
            other => panic!("{name}: expected UnsupportedCodec, got {other:?}"),
        }
    }
}

#[test]
fn ambisonics_mono_reference_element_parses() {
    // The 3OA file's scene element uses mono ambisonics mode (no coupled
    // count, per-channel mapping) alongside a stereo channel element.
    let bytes = data("tones_100ms_3OA_stereo_opus.iamf");
    let (d, _) = parse_descriptors(&bytes).expect("parse 3OA descriptors");
    assert_eq!(d.audio_elements.len(), 2);
    match &d.audio_elements[0].element_config {
        sotf_iamf::types::ElementConfig::Scene(cfg) => {
            assert_eq!(cfg.coupled_substream_count, 0);
            assert_eq!(cfg.channel_mapping.len(), cfg.output_channel_count as usize);
        }
        other => panic!("expected scene element, got {other:?}"),
    }
    match &d.audio_elements[1].element_config {
        sotf_iamf::types::ElementConfig::Channel(_) => {}
        other => panic!("expected channel element, got {other:?}"),
    }
}
