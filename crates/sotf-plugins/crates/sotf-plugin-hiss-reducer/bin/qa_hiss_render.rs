//! Render explicit noise captures and native hiss reduction for offline proposals.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::param_specs::find_by_key;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
// Includes eight-channel measured profiles; limits file bytes, not process RSS.
const MAX_REQUEST_BYTES: usize = 262_144;
const MAX_SAMPLES: usize = 4_000_000;
const MAX_DRAIN_FRAMES: usize = 65_536;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    capture_input: Option<PathBuf>,
    sample_rate: u32,
    channels: usize,
    frames: usize,
    callback_frames: usize,
    params: HissReducerPluginParams,
}

fn read_audio(path: &Path, samples: usize) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((samples * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != samples * 4 {
        return Err("PCM size must exactly match frame/channel geometry".into());
    }
    let audio: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if audio.iter().any(|v| !v.is_finite() || v.abs() > 32.0) {
        return Err("use finite PCM within +/-32".into());
    }
    Ok(audio)
}

fn process(
    plugin: &mut HissReducerPlugin,
    audio: &[f32],
    request: &Request,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let frames = audio.len() / request.channels;
    let mut output = Vec::with_capacity(audio.len());
    let mut block = vec![0.0; request.callback_frames * request.channels];
    for position in (0..frames).step_by(request.callback_frames) {
        let count = (frames - position).min(request.callback_frames);
        let samples = count * request.channels;
        block[..samples].copy_from_slice(
            &audio[position * request.channels..position * request.channels + samples],
        );
        let context =
            ProcessContext::new(request.sample_rate, count).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("hiss native capture/program process", || {
            result = plugin.process_in_place(&mut block[..samples], &context);
        });
        if result? != count {
            return Err("native processor changed accepted frame count".into());
        }
        output.extend_from_slice(&block[..samples]);
    }
    Ok(output)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-hiss-render REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("request exceeds 256 KiB".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let samples = request
        .frames
        .checked_mul(request.channels)
        .ok_or("sample count overflow")?;
    if !(44_100..=192_000).contains(&request.sample_rate)
        || !(1..=8).contains(&request.channels)
        || samples == 0
        || samples > MAX_SAMPLES
        || !(1..=4096).contains(&request.callback_frames)
        || (request.capture_input.is_some() && request.params.captured_profile.is_some())
    {
        return Err("unsupported geometry or conflicting capture/profile inputs".into());
    }
    // The production loader intentionally canonicalizes controls. Qualification
    // rejects out-of-range requests so clamping cannot disguise the audition.
    for (key, value) in [
        ("threshold_db", request.params.threshold_db),
        ("frequency_hz", request.params.frequency_hz),
        ("strength", request.params.strength),
        ("curve_low", request.params.curve_low),
        ("curve_mid", request.params.curve_mid),
        ("curve_high", request.params.curve_high),
    ] {
        let spec = find_by_key(sotf_plugin_hiss_reducer::params::PARAMS, key);
        let maximum = if key == "frequency_hz" {
            spec.max_f64().min(f64::from(request.sample_rate) * 0.45)
        } else {
            spec.max_f64()
        };
        if !value.is_finite() || !(spec.min_f64()..=maximum).contains(&f64::from(value)) {
            return Err(format!("invalid explicit parameter {key}").into());
        }
    }
    if let Some(profile) = &request.params.captured_profile {
        profile.validate()?;
        if profile.channels != request.channels
            || profile.sample_rate != f64::from(request.sample_rate)
            || profile.measurement_cutoff_hz != request.params.frequency_hz
        {
            return Err(
                "profile must match explicit channels, rate and cutoff; no silent drop/fallback"
                    .into(),
            );
        }
    }
    let input = read_audio(&request.input, samples)?;
    let mut effective = request.params.clone();
    if let Some(path) = &request.capture_input {
        let capture = read_audio(path, request.sample_rate as usize * request.channels)?;
        let mut measuring = HissReducerPlugin::try_from_params_at_sample_rate(
            request.channels,
            request.sample_rate,
            effective,
        )?;
        measuring.initialize(f64::from(request.sample_rate))?;
        let learn_id = sotf_host::parameters::ParameterId::from("learn_noise");
        let mut trigger = Ok(());
        assert_no_allocs_or_deallocs("hiss native learn trigger", || {
            trigger =
                measuring.parametric_set_parameter(learn_id.clone(), ParameterValue::Bool(true));
        });
        trigger?;
        process(&mut measuring, &capture, &request)?;
        if measuring.is_capturing() || !measuring.has_captured_profile() {
            return Err("native one-second capture did not complete".into());
        }
        effective = measuring.persisted_params();
        let profile = effective
            .captured_profile
            .as_ref()
            .ok_or("completed capture has no profile")?;
        if profile.frames_analyzed != u64::from(request.sample_rate) {
            return Err(
                "native capture duration differs from the explicit one-second interval".into(),
            );
        }
    }
    // A separate fresh processor excludes captured audio/filter/envelope history.
    let mut plugin = HissReducerPlugin::try_from_params_at_sample_rate(
        request.channels,
        request.sample_rate,
        effective,
    )?;
    plugin.initialize(f64::from(request.sample_rate))?;
    let latency = plugin.latency_samples();
    let mut output = process(&mut plugin, &input, &request)?;
    let capacity = plugin.drain_output_frames_max();
    let bound = plugin
        .drain_call_bound()
        .ok_or("missing public drain bound")?
        .get();
    if capacity > MAX_DRAIN_FRAMES || bound > MAX_DRAIN_FRAMES as u64 {
        return Err("native drain exceeds offline bounds".into());
    }
    let mut tail = vec![0.0; capacity * request.channels];
    let mut drained = 0;
    let mut complete = false;
    for _ in 0..bound {
        let mut result = None;
        assert_no_allocs_or_deallocs("hiss native program drain", || {
            result = Some(plugin.drain(&mut tail, &ProcessContext::new(request.sample_rate, 0)));
        });
        let result = result.expect("drain closure ran")?;
        if result.frames > capacity {
            return Err("native drain exceeded its destination capacity".into());
        }
        drained += result.frames;
        if drained > MAX_DRAIN_FRAMES {
            return Err("native drain exceeds frame limit".into());
        }
        output.extend_from_slice(&tail[..result.frames * request.channels]);
        if result.complete {
            complete = true;
            break;
        }
    }
    if !complete
        || drained < latency
        || output.len() != (request.frames + drained) * request.channels
        || output.iter().any(|v| !v.is_finite())
    {
        return Err("invalid native output or drain alignment".into());
    }
    let effective = plugin.persisted_params();
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&request.output)?,
    );
    for sample in output {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    println!(
        "{}",
        json!({"status":"ok","sample_rate":request.sample_rate,"channels":request.channels,
        "input_frames":request.frames,"output_frames":request.frames+drained,"drained_frames":drained,
        "latency_samples":latency,"callback_frames":request.callback_frames,"capture_frames":if request.capture_input.is_some(){request.sample_rate}else{0},
        "effective_params":effective,"measured_profile_applicable":plugin.profile_uses_spectral(),
        "scope":"Explicit one-second native capture or supplied profile; fresh programme state; full public spectral drain; conventional IIR mode has unknown tail/no drain. No learned classification or automatic capture consent."})
    );
    Ok(())
}
