//! Audition offline speaker corrections through the unchanged native decoder.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::{Value, json};
use sotf_host::speaker_config::get_speaker_config;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_ambisonics::custom_layout::{CustomDecoderConfig, CustomLayout, CustomSpeaker};
use sotf_plugin_ambisonics::decode_matrix::DecodeMatrix;
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};
use sotf_plugin_matrix::MatrixPlugin;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
// Bound the combined input/output working set, including observation zeros.
const MAX_SAMPLES: usize = 4_000_000;
const MAX_ZERO_FRAMES: usize = 65_536;
const MAX_REQUEST_BYTES: usize = 65_536;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    sample_rate: u32,
    frames: usize,
    callback_frames: usize,
    appended_zero_frames: usize,
    normalization: String,
    params: Value,
    speaker_gains: Vec<f32>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-ambisonics-render REQUEST.json")?;
    let mut raw = Vec::new();
    File::open(path)?
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() > MAX_REQUEST_BYTES {
        return Err("request exceeds64KiB".into());
    }
    let request: Request = serde_json::from_slice(&raw)?;
    if request.normalization != "ACN/SN3D"
        || !(44100..=192000).contains(&request.sample_rate)
        || request.frames == 0
        || !(1..=4096).contains(&request.callback_frames)
        || request.appended_zero_frames > MAX_ZERO_FRAMES
    {
        return Err("unsupported normalization or bounded render geometry".into());
    }
    let object = request
        .params
        .as_object()
        .ok_or("supply explicit decoder controls")?;
    let custom = object.get("target_layout").and_then(Value::as_str) == Some("custom");
    let keys = [
        "order",
        "target_layout",
        "max_re_weighting",
        "dual_band",
        "algorithm",
    ];
    if object.len() != keys.len() + usize::from(custom)
        || keys.iter().any(|key| !object.contains_key(*key))
        || (custom && !object.contains_key("custom_layout"))
    {
        return Err("supply the exact complete native decoder schema".into());
    }
    let config: AmbisonicsDecoderConfig = serde_json::from_value(request.params.clone())?;
    // The native constructor validates order before computing its channel count.
    if !(1..=7).contains(&config.order) {
        return Err("unsupported Ambisonics order".into());
    }
    if !sotf_plugin_ambisonics::params::TARGET_LAYOUTS.contains(&config.target_layout.as_str()) {
        return Err("layout is not a registered native decoder choice".into());
    }
    let layout = if custom {
        let geometry = object["custom_layout"]
            .as_object()
            .ok_or("invalid custom layout")?;
        if geometry.len() != 2
            || !geometry.contains_key("name")
            || !geometry.contains_key("speakers")
        {
            return Err("unexpected custom layout fields".into());
        }
        for speaker in geometry["speakers"].as_array().ok_or("invalid speakers")? {
            let fields = speaker.as_object().ok_or("invalid speaker")?;
            let keys = ["label", "azimuth_deg", "elevation_deg", "is_lfe"];
            if fields.len() != keys.len() || keys.iter().any(|key| !fields.contains_key(*key)) {
                return Err("unexpected custom speaker fields".into());
            }
        }
        serde_json::from_value::<CustomDecoderConfig>(request.params.clone())?.custom_layout
    } else {
        let named = get_speaker_config(&config.target_layout).ok_or("unknown named layout")?;
        let mut speakers = vec![None; named.total_channels];
        for speaker in named.speakers {
            speakers[speaker.channel] = Some(CustomSpeaker {
                label: speaker.label.to_owned(),
                azimuth_deg: speaker.azimuth,
                elevation_deg: speaker.elevation,
                is_lfe: speaker.is_lfe,
            });
        }
        CustomLayout {
            name: named.id.to_owned(),
            speakers: speakers
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or("incomplete named layout")?,
        }
    };
    layout.validate()?;
    let effective_params = if custom {
        serde_json::to_value(CustomDecoderConfig {
            params: config.clone(),
            custom_layout: layout.clone(),
        })?
    } else {
        serde_json::to_value(&config)?
    };
    let inputs = (config.order + 1).pow(2);
    let outputs = layout.speakers.len();
    let frames = request
        .frames
        .checked_add(request.appended_zero_frames)
        .ok_or("frame overflow")?;
    if frames
        .checked_mul(inputs + outputs)
        .ok_or("sample overflow")?
        > MAX_SAMPLES
        || request.speaker_gains.len() != outputs
        || request
            .speaker_gains
            .iter()
            .zip(&layout.speakers)
            .any(|(gain, speaker)| {
                !gain.is_finite()
                    || !(0.8..=1.25).contains(gain)
                    || (speaker.is_lfe && *gain != 1.0)
            })
    {
        return Err("unsupported combined sample count or bounded speaker correction".into());
    }
    let mut decoder = if custom {
        AmbisonicsDecoderPlugin::new_custom(&CustomDecoderConfig {
            params: config.clone(),
            custom_layout: layout.clone(),
        })?
    } else {
        AmbisonicsDecoderPlugin::new(&config)?
    };
    let build = |weighted| {
        if custom {
            match config.algorithm.as_str() {
                "mode_matching" => DecodeMatrix::build_for_custom(config.order, &layout, weighted),
                "allrad" => DecodeMatrix::build_allrad_for_custom(config.order, &layout, weighted),
                _ => Err("unknown algorithm".into()),
            }
        } else {
            let named = get_speaker_config(&config.target_layout).ok_or("unknown layout")?;
            match config.algorithm.as_str() {
                "mode_matching" => DecodeMatrix::build(config.order, named, weighted),
                "allrad" => DecodeMatrix::build_allrad(config.order, named, weighted),
                _ => Err("unknown algorithm".into()),
            }
        }
    };
    let matrix = build(config.max_re_weighting)?.export();
    let basic = if config.dual_band {
        Some(build(false)?.export())
    } else {
        None
    };
    let mut diagonal = vec![0.0; outputs * outputs];
    for (channel, gain) in request.speaker_gains.iter().enumerate() {
        diagonal[channel * outputs + channel] = *gain;
    }
    let map: Vec<_> = (0..outputs).collect();
    let matrix_params = json!({"input_channel_map":map,"output_channel_map":map,"matrix":diagonal});
    let mut correction = MatrixPlugin::with_sparse_mapping(map.clone(), map, diagonal)?;
    decoder.initialize(request.sample_rate.into())?;
    correction.initialize(request.sample_rate.into())?;
    assert_no_allocs_or_deallocs("Ambisonics and correction reset", || {
        decoder.reset();
        correction.reset();
    });
    if decoder.latency_samples() != 0 || correction.latency_samples() != 0 {
        return Err("unexpected native latency".into());
    }
    let samples = request.frames * inputs;
    let mut bytes = Vec::new();
    File::open(&request.input)?
        .take((samples * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != samples * 4 {
        return Err("PCM byte count mismatch".into());
    }
    let input: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if input.iter().any(|v| !v.is_finite() || v.abs() > 32.0) {
        return Err("input must be finite within+/-32".into());
    }
    let mut output = Vec::with_capacity(frames * outputs);
    let mut block = vec![0.0; request.callback_frames * inputs];
    let mut decoded = vec![0.0; request.callback_frames * outputs];
    let mut corrected = vec![0.0; request.callback_frames * outputs];
    for position in (0..frames).step_by(request.callback_frames) {
        let count = (frames - position).min(request.callback_frames);
        block[..count * inputs].fill(0.0);
        let real = request.frames.saturating_sub(position).min(count) * inputs;
        if real > 0 {
            block[..real].copy_from_slice(&input[position * inputs..position * inputs + real]);
        }
        decoded[..count * outputs].fill(f32::NAN);
        corrected[..count * outputs].fill(f32::NAN);
        let context =
            ProcessContext::new(request.sample_rate, count).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("Ambisonics decoder process", || {
            result = decoder.process(
                &block[..count * inputs],
                &mut decoded[..count * outputs],
                &context,
            );
        });
        if result? != count {
            return Err("decoder frame count mismatch".into());
        }
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("Ambisonics speaker correction process", || {
            result = correction.process(
                &decoded[..count * outputs],
                &mut corrected[..count * outputs],
                &context,
            );
        });
        if result? != count {
            return Err("correction frame count mismatch".into());
        }
        output.extend_from_slice(&corrected[..count * outputs]);
    }
    if output.iter().any(|v| !v.is_finite()) {
        return Err("non-finite native output".into());
    }
    let mut file = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&request.output)?,
    );
    for sample in output {
        file.write_all(&sample.to_le_bytes())?;
    }
    file.flush()?;
    println!(
        "{}",
        json!({"status":"ok", "normalization":request.normalization,
        "sample_rate":request.sample_rate,"input_channels":inputs,"output_channels":outputs,
        "input_frames":request.frames,"output_frames":frames,"callback_frames":request.callback_frames,
        "appended_zero_frames":request.appended_zero_frames,"latency_samples":0,
        "tail":if config.dual_band { "unknown_recursive" } else { "finite_zero" },
        "effective_params":effective_params,"speaker_layout":layout,"speaker_gains":request.speaker_gains,
        "decoder_matrix":matrix,"basic_matrix":basic,"correction_params":matrix_params,
        "scope":"Native decoder followed by native diagonal Matrix correction. No normalization, limiting, inference or automatic changes. Explicit zeros do not establish a finite dual-band tail. Wider custom layouts are DSP-only until host admission is validated."})
    );
    Ok(())
}
