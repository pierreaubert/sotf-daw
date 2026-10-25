//! JSON-lines offline impulse renderer for optional learned room fitting.
//!
//! The observation window is caller-selected and never implies finite native
//! support. Small impulses, disabled modulation/adaptation, and explicit fixed
//! profile controls make this an identification tool, separate from audio QA.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sotf_host::{CountingAlloc, Plugin, ProcessContext, TailLength, assert_no_allocs};
use sotf_plugin_aae::{AaePlugin, params::AaePluginParams};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufWriter, Write};
use std::path::Path;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    params: AaePluginParams,
    sample_rate: u32,
    frames: usize,
    partition: usize,
    excitation: f32,
}

fn validate(request: &Request) -> Result<(), String> {
    if request.id.is_empty()
        || request.id.len() > 80
        || !request
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("id must contain 1..80 ASCII letters, digits, underscores or hyphens".into());
    }
    if !(16_000..=192_000).contains(&request.sample_rate)
        || request.frames == 0
        || request.frames > request.sample_rate as usize * 32
        || !(1..=16_384).contains(&request.partition)
    {
        return Err(
            "render requires 16..192 kHz, 1..32 seconds of frames, and partition 1..16384".into(),
        );
    }
    if !request.excitation.is_finite() || !(0.00001..=0.001).contains(&request.excitation) {
        return Err("identification excitation must be between 0.00001 and 0.001".into());
    }
    let p = &request.params;
    if p.dry_level != 0.0
        || p.lfe_level != 0.0
        || p.mod_depth != 0.0
        || p.er_mod_depth != 0.0
        || p.content_aware
        || p.auto_gain_enabled
        || p.bypass
        || p.solo_early
        || p.solo_late
    {
        return Err(
            "identification requires wet-only, unmodulated, non-adaptive normal rendering".into(),
        );
    }
    Ok(())
}

fn render(root: &Path, request: &Request) -> Result<Value, String> {
    validate(request)?;
    let mut plugin = AaePlugin::try_from_params(request.params.clone())?;
    plugin.initialize(f64::from(request.sample_rate))?;
    let channels = plugin.output_channels();
    let path = root.join(format!("{}.f32le", request.id));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let mut writer = BufWriter::new(file);
    let mut input = vec![0.0; request.partition * 2];
    let mut output = vec![0.0; request.partition * channels];
    let mut bytes = vec![0_u8; request.partition * channels * 4];
    let mut hash = Sha256::new();
    let mut energy = vec![0.0_f64; channels];
    let mut peak = 0.0_f32;
    let mut position = 0;
    while position < request.frames {
        let frames = request.partition.min(request.frames - position);
        input.fill(0.0);
        if position == 0 {
            input[..2].fill(request.excitation);
        }
        let context =
            ProcessContext::new(request.sample_rate, frames).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs("AAE fitting reference callback", || {
            result = plugin.process(
                &input[..frames * 2],
                &mut output[..frames * channels],
                &context,
            );
        });
        let consumed = result?;
        if consumed != frames {
            return Err("AAE fitting reference changed frame count".into());
        }
        for (index, &value) in output[..frames * channels].iter().enumerate() {
            if !value.is_finite() {
                return Err("AAE fitting reference produced non-finite output".into());
            }
            peak = peak.max(value.abs());
            energy[index % channels] += f64::from(value).powi(2);
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        let bytes = &bytes[..frames * channels * 4];
        hash.update(bytes);
        writer.write_all(bytes).map_err(|e| e.to_string())?;
        position += frames;
    }
    writer.flush().map_err(|e| e.to_string())?;
    if !matches!(plugin.tail_length(), TailLength::Infinite) {
        return Err("accepted excitation lost conservative infinite-tail metadata".into());
    }
    let mut sha256 = String::with_capacity(64);
    for byte in hash.finalize() {
        write!(sha256, "{byte:02x}").map_err(|e| e.to_string())?;
    }
    Ok(
        json!({"id":request.id,"status":"ok","file":path,"format":"interleaved_f32le",
        "sample_rate":request.sample_rate,"frames":request.frames,"channels":channels,
        "partition":request.partition,"excitation":request.excitation,"peak":peak,
        "channel_energy":energy,"sha256":sha256,
        "native_tail":"Infinite","observation_truncated":true,"output_fade":false,
        "params":request.params}),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-aae-fit-render OUTPUT_DIRECTORY < requests.jsonl")?;
    let root = Path::new(&root);
    fs::create_dir_all(root)?;
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let line = line?;
        let result = match serde_json::from_str::<Request>(&line) {
            Ok(request) => render(root, &request)
                .unwrap_or_else(|error| json!({"id":request.id,"status":"error","error":error})),
            Err(error) => json!({"status":"error","error":error.to_string()}),
        };
        serde_json::to_writer(&mut output, &result)?;
        writeln!(output)?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, partition: usize) -> Request {
        Request {
            id: id.into(),
            params: AaePluginParams {
                speaker_config: "5.0".into(),
                dry_level: 0.0,
                lfe_level: 0.0,
                mod_depth: 0.0,
                er_mod_depth: 0.0,
                content_aware: false,
                auto_gain_enabled: false,
                ..Default::default()
            },
            sample_rate: 16_000,
            frames: 4099,
            partition,
            excitation: 0.001,
        }
    }

    #[test]
    fn identification_render_is_exact_across_partitions_and_retains_infinite_support() {
        let root = std::env::temp_dir().join(format!("aae_fit_render_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut first = None;
        for partition in [1, 257, 1024] {
            let id = format!("p{partition}");
            let result = render(&root, &request(&id, partition)).unwrap();
            assert_eq!(result["native_tail"], "Infinite");
            assert_eq!(result["frames"], 4099);
            assert!(result["peak"].as_f64().unwrap() > 0.0);
            if let Some(hash) = &first {
                assert_eq!(result["sha256"], *hash);
            } else {
                first = Some(result["sha256"].clone());
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn identification_rejects_adaptation_modulation_and_path_escape_before_output() {
        let mut r = request("../escape", 257);
        assert!(validate(&r).is_err());
        r.id = "valid".into();
        r.params.content_aware = true;
        assert!(validate(&r).is_err());
        r.params.content_aware = false;
        r.params.mod_depth = 0.1;
        assert!(validate(&r).is_err());
        r.params.mod_depth = 0.0;
        r.frames = 0;
        assert!(validate(&r).is_err());
    }
}
