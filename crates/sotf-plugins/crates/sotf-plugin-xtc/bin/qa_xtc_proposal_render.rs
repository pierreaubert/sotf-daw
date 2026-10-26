//! Render complete native XTC output and export its actual prepared matrices.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_xtc::learned_proposal::PreparedProposal;
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalFile {
    path: PathBuf,
    sha256: String,
    model_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    metadata: PathBuf,
    frames: usize,
    callback_frames: usize,
    proposal: Option<ProposalFile>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-xtc-proposal-render REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&bytes)?;
    if !(1..=1_000_000).contains(&r.frames) || !(1..=1024).contains(&r.callback_frames) {
        return Err("invalid bounded XTC render geometry".into());
    }
    let params = XtcPluginParams {
        distance_m: 1.4,
        speaker_angle_deg: 30.0,
        auto_gain_enabled: false,
        ..Default::default()
    };
    let mut p = XtcPlugin::new(params.clone(), 44100)?;
    p.initialize(44100.0)?;
    let mut proposal_error = None;
    if let Some(proposal) = &r.proposal {
        let loaded = (|| -> Result<(), String> {
            let mut raw = Vec::new();
            File::open(&proposal.path)
                .map_err(|e| e.to_string())?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut raw)
                .map_err(|e| e.to_string())?;
            let prepared =
                PreparedProposal::from_json(&raw, &proposal.sha256, &proposal.model_sha256)?;
            p.install_learned_proposal(prepared)
        })();
        if let Err(e) = loaded {
            proposal_error = Some(e);
        }
    }
    assert_no_allocs_or_deallocs("XTC proposal renderer reset", || p.reset());
    let coefficients: Vec<_> = (0..1025)
        .map(|b| {
            std::array::from_fn::<_, 2, _>(|s| {
                std::array::from_fn::<_, 2, _>(|i| {
                    p.prepared_filter_coefficient(b, s, i)
                        .expect("fixed stereo geometry")
                })
            })
        })
        .collect();
    bytes.clear();
    File::open(&r.input)?
        .take((r.frames * 8 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != r.frames * 8 {
        return Err("source size mismatch".into());
    }
    let input: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    if input.iter().any(|v| !v.is_finite() || v.abs() > 64.0) {
        return Err("source exceeds bounded domain".into());
    }
    let mut block = vec![0.0; r.callback_frames * 2];
    let mut output = Vec::with_capacity((r.frames + 65536) * 2);
    for chunk in input.chunks(r.callback_frames * 2) {
        let frames = chunk.len() / 2;
        let mut done = Ok(0);
        assert_no_allocs_or_deallocs("XTC proposal renderer process", || {
            done = p.process(
                chunk,
                &mut block[..frames * 2],
                &ProcessContext::new(44100, frames),
            );
        });
        if done? != frames {
            return Err("native source frame mismatch".into());
        }
        output.extend_from_slice(&block[..frames * 2]);
    }
    let mut complete = false;
    for _ in 0..65536 {
        let mut drained = None;
        assert_no_allocs_or_deallocs("XTC proposal renderer drain", || {
            drained = Some(p.drain(&mut block, &ProcessContext::new(44100, r.callback_frames)));
        });
        let d = drained.ok_or("missing drain")??;
        output.extend_from_slice(&block[..d.frames * 2]);
        if output.len() / 2 > r.frames + 65536 {
            return Err("native XTC tail exceeds export bound".into());
        }
        if d.complete {
            complete = true;
            break;
        }
    }
    if !complete || output.iter().any(|v| !v.is_finite()) {
        return Err("invalid native XTC output/tail".into());
    }
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&r.output)?,
    );
    for value in &output {
        writer.write_all(&value.to_le_bytes())?;
    }
    writer.flush()?;
    let metadata = json!({"sample_rate":44100,"fft_size":2048,"hop_size":512,"source_frames":r.frames,"output_frames":output.len()/2,
        "latency_samples":p.latency_samples(),"model_lookahead_samples":0,"parameters":params,"analysis_window":p.proposal_analysis_window(),
        "output_scale":p.proposal_output_scale(),"filters_real_imag":coefficients,"selected_proposal_sha256":p.learned_proposal_sha256(),
        "proposal_error":proposal_error,"deployment_admitted":false});
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&r.metadata)?;
    serde_json::to_writer(&mut file, &metadata)?;
    file.write_all(b"\n")?;
    println!(
        "{}",
        json!({"source_frames":r.frames,"output_frames":output.len()/2,"latency_samples":p.latency_samples(),"proposal_selected":p.learned_proposal_sha256().is_some()})
    );
    Ok(())
}
