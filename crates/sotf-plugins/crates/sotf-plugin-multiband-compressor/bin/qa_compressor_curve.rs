//! Measure native isolated-band static transfer from an explicit full preset.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext, assert_no_allocs};
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};
use std::fs::File;
use std::io::Read;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    sample_rate: u32,
    callback_frames: usize,
    settle_multiple: f64,
    params: MultibandCompressorPluginParams,
    levels_dbfs: Vec<Vec<f64>>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-compressor-curve REQUEST.json")?;
    let mut bytes = Vec::new();
    // Five bands, 128 levels each and native parameters fit well below this cap.
    File::open(path)?.take(262_145).read_to_end(&mut bytes)?;
    if bytes.len() > 262_144 {
        return Err("request exceeds QA bound".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let p = &request.params;
    if !(44_100..=192_000).contains(&request.sample_rate)
        || !(1..=4_096).contains(&request.callback_frames)
        || !request.settle_multiple.is_finite()
        || !(16.0..=64.0).contains(&request.settle_multiple)
        || !(1..=5).contains(&p.num_bands)
        || p.bands.len() != p.num_bands
        || request.levels_dbfs.len() != p.num_bands
        || p.mix != 1.0
        || p.ms_mode
        || p.sidechain_tilt_db != 0.0
        || p.per_band_lookahead_ms != 0.0
        || p.lookahead_ms.unwrap_or(0.0) != 0.0
        || p.sidechain_hpf_enabled.unwrap_or(false)
        || p.sidechain_external.unwrap_or(false)
        || p.program_dependent_release.unwrap_or(false)
        || p.makeup_gain.is_some()
        || p.auto_makeup.unwrap_or(false)
        || p.measured_auto_makeup.unwrap_or(false)
        || p.detection_mode
            .as_deref()
            .is_some_and(|mode| !mode.eq_ignore_ascii_case("peak"))
        || p.range_db != 120.0
        || p.hold_ms != 0.0
    {
        return Err("unsupported bounded static measurement profile".into());
    }
    for (band, levels) in p.bands.iter().zip(&request.levels_dbfs) {
        if band.auto_makeup
            || band.measured_auto_makeup
            || !band.active
            || band.solo
            || band.bypass
            || band.range_db.unwrap_or(120.0) != 120.0
            || band.hold_ms.unwrap_or(0.0) != 0.0
            || levels.is_empty()
            || levels.len() > 128
            || levels
                .iter()
                .any(|x| !x.is_finite() || !(-120.0..=0.0).contains(x))
        {
            return Err("unsupported native band or plateau levels".into());
        }
    }
    let mut full = MultibandCompressorPlugin::try_from_params(2, p.clone(), request.sample_rate)?;
    full.initialize(f64::from(request.sample_rate))?;
    let full_latency = full.latency_samples();
    let mut smoke = vec![0.0_f32; 386];
    smoke[0] = 0.1;
    smoke[1] = -0.05;
    let mut result = Ok(0);
    assert_no_allocs("full compressor preset smoke", || {
        result = full.process_in_place(&mut smoke, &ProcessContext::new(request.sample_rate, 193));
    });
    if result? != 193 || smoke.iter().any(|x| !x.is_finite()) {
        return Err("invalid full preset smoke output".into());
    }
    let mut curves = Vec::new();
    for (band_index, (band, levels)) in p.bands.iter().zip(&request.levels_dbfs).enumerate() {
        let mut isolated = p.clone();
        isolated.num_bands = 1;
        isolated.crossover_frequencies.clear();
        isolated.bands = vec![band.clone()];
        let mut plugin =
            MultibandCompressorPlugin::try_from_params(1, isolated, request.sample_rate)?;
        plugin.initialize(f64::from(request.sample_rate))?;
        let attack = f64::from(band.attack_ms.unwrap_or(p.attack_ms));
        let frames = (f64::from(request.sample_rate)
            * (request.settle_multiple * attack / 1000.0 + 0.05))
            .ceil() as usize;
        let mut block = vec![0.0_f32; request.callback_frames];
        let mut measured = Vec::with_capacity(levels.len());
        for level in levels {
            plugin.reset();
            let amplitude = 10.0_f64.powf(level / 20.0) as f32;
            let mut position = 0;
            let mut final_mean = 0.0;
            while position < frames {
                let count = (frames - position).min(block.len());
                block[..count].fill(amplitude);
                let context = ProcessContext::new(request.sample_rate, count)
                    .with_sample_position(position as u64);
                let mut processed = Ok(0);
                assert_no_allocs("isolated compressor plateau", || {
                    processed = plugin.process_in_place(&mut block[..count], &context);
                });
                if processed? != count || block[..count].iter().any(|x| !x.is_finite() || *x <= 0.0)
                {
                    return Err("invalid native plateau output".into());
                }
                let tail = count.min(256);
                final_mean = block[count - tail..count]
                    .iter()
                    .map(|x| f64::from(*x))
                    .sum::<f64>()
                    / tail as f64;
                position += count;
            }
            measured.push(20.0 * final_mean.log10());
        }
        curves.push(json!({"band":band_index,"input_dbfs":levels,"output_dbfs":measured,"plateau_frames":frames}));
    }
    println!(
        "{}",
        json!({"status":"ok","sample_rate":request.sample_rate,
        "callback_frames":request.callback_frames,"settle_multiple":request.settle_multiple,
        "full_preset_latency_samples":full_latency,"curves":curves,
        "scope":"isolated native band gain curves plus full-preset construction/finite smoke; not mixed-band or temporal target matching"})
    );
    Ok(())
}
