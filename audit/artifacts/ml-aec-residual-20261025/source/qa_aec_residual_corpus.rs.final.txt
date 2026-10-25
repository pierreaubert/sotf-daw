//! Export native adaptive AEC observations and independent near-end oracle gains.
//!
//! JSON-lines requests reference interleaved mic/reference and mono near-end
//! f32le files. Every request owns fresh adaptive state. Output is analysis data,
//! not model-assisted audio; the existing classical output is exported separately.

// Rust guideline compliant 2026-02-21
use rustfft::{FftPlanner, num_complex::Complex};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sotf_host::{CountingAlloc, Plugin, ProcessContext, assert_no_allocs};
use sotf_plugin_aec::residual_features::{
    BANDS, BINS, FFT_SIZE, ResidualFeatureExtractor, band_range,
};
use sotf_plugin_aec::{AecPlugin, AecPluginParams};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    sample_rate: u32,
    frames: usize,
    input: PathBuf,
    near: PathBuf,
}

struct Output {
    writer: BufWriter<std::fs::File>,
    hash: Sha256,
    floats: usize,
}

impl Output {
    fn new(path: &Path) -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(OpenOptions::new().create_new(true).write(true).open(path)?),
            hash: Sha256::new(),
            floats: 0,
        })
    }

    fn append(&mut self, values: impl IntoIterator<Item = f32>) -> io::Result<()> {
        for value in values {
            let bytes = value.to_le_bytes();
            self.hash.update(bytes);
            self.writer.write_all(&bytes)?;
            self.floats += 1;
        }
        Ok(())
    }

    fn finish(mut self) -> io::Result<Value> {
        self.writer.flush()?;
        let mut hash = String::with_capacity(64);
        for byte in self.hash.finalize() {
            write!(hash, "{byte:02x}").expect("writing to String cannot fail");
        }
        Ok(json!({"sha256": hash, "floats": self.floats}))
    }
}

fn read_samples(path: &Path, count: usize) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut raw = Vec::new();
    std::fs::File::open(path)?
        .take((count * 4 + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() != count * 4 {
        return Err("input sample count mismatch".into());
    }
    let samples: Vec<f32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect();
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err("non-finite corpus source".into());
    }
    Ok(samples)
}

fn export(root: &Path, request: &Request) -> Result<Value, Box<dyn std::error::Error>> {
    if request.id.is_empty()
        || request.id.len() > 80
        || !request
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        || !(16_000..=48_000).contains(&request.sample_rate)
        || request.frames == 0
        || request.frames > request.sample_rate as usize * 12
        || !request.frames.is_multiple_of(256)
    {
        return Err("invalid corpus id, rate or bounded whole-block frame count".into());
    }
    let input = read_samples(&request.input, request.frames * 2)?;
    let near = read_samples(&request.near, request.frames)?;
    let path = root.join(&request.id);
    fs::create_dir(&path)?;
    let mut features = Output::new(&path.join("features.f32le"))?;
    let mut targets = Output::new(&path.join("targets.f32le"))?;
    let mut error_spectra = Output::new(&path.join("error_spectra.f32le"))?;
    let mut classical = Output::new(&path.join("classical.f32le"))?;
    let mut adaptive = Output::new(&path.join("adaptive.f32le"))?;
    let mut extractor = ResidualFeatureExtractor::new(f64::from(request.sample_rate))?;
    let mut plugin = AecPlugin::from_params(
        request.sample_rate,
        AecPluginParams {
            echo_tail_ms: 50.0,
            step_size: 0.5,
            post_filter_enabled: true,
        },
    )?;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut fft_scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    let mut near_spectrum = [Complex::default(); FFT_SIZE];
    let mut output = [0.0; 256];
    for block in 0..request.frames / 256 {
        let first = block * 256;
        let context =
            ProcessContext::new(request.sample_rate, 256).with_sample_position(first as u64);
        let mut processed = Ok(0);
        assert_no_allocs("native AEC residual corpus callback", || {
            processed = plugin.process(&input[first * 2..(first + 256) * 2], &mut output, &context);
        });
        if processed? != 256 {
            return Err("AEC changed frame count".into());
        }
        // Exactly one queued block of latency. Save previous output now, then
        // drain the final real block after collection, without adapting on EOF.
        if block != 0 {
            classical.append(output)?;
        }
        let analysis = plugin
            .residual_training_analysis()
            .ok_or("missing native analysis endpoint")?;
        let power = |channel: usize| {
            input[first * 2..(first + 256) * 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|x| f64::from(x[channel]).powi(2))
                .sum::<f64>()
                / 256.0
        };
        let frame = extractor
            .observe(
                analysis.error_spectrum,
                analysis.echo_spectrum,
                power(0),
                power(1),
            )
            .ok_or("invalid native residual observation")?;
        features.append(frame.into_iter().flatten())?;
        error_spectra.append(analysis.error_spectrum.iter().flat_map(|x| [x.re, x.im]))?;
        adaptive.append(analysis.error.iter().copied())?;
        near_spectrum.fill(Complex::default());
        for (out, &sample) in near_spectrum[256..]
            .iter_mut()
            .zip(&near[first..first + 256])
        {
            out.re = sample;
        }
        fft.process_with_scratch(&mut near_spectrum, &mut fft_scratch);
        let mut gains = [1.0; BANDS];
        let mut weights = [1.0; BANDS];
        for band in 0..BANDS {
            let (mut cross, mut error_power, mut near_power) = (0.0_f64, 0.0_f64, 0.0_f64);
            for k in band_range(band) {
                let weight = if k == 0 || k == BINS - 1 { 1.0 } else { 2.0 };
                let e = analysis.error_spectrum[k];
                let s = near_spectrum[k];
                cross += weight
                    * (f64::from(s.re) * f64::from(e.re) + f64::from(s.im) * f64::from(e.im));
                error_power += weight * (f64::from(e.re).powi(2) + f64::from(e.im).powi(2));
                near_power += weight * (f64::from(s.re).powi(2) + f64::from(s.im).powi(2));
            }
            // Least-squares real mask minimizes complex near-end error for
            // this band; target is known clean near-end, never mixture power.
            gains[band] = if error_power > 1e-20 {
                (cross / error_power).clamp(0.25, 1.0) as f32
            } else {
                1.0
            };
            // Preservation-weighted training; cap leverage for quiet bins.
            weights[band] =
                (1.0 + 9.0 * (near_power / error_power.max(1e-20)).clamp(0.0, 1.0)) as f32;
        }
        targets.append(gains.into_iter().chain(weights))?;
    }
    let drain = plugin.drain(&mut output, &ProcessContext::new(request.sample_rate, 256))?;
    if drain.frames != 256 {
        return Err("missing queued final AEC block".into());
    }
    classical.append(output)?;
    Ok(
        json!({"status":"ok", "id":request.id, "directory":path, "sample_rate":request.sample_rate,
        "frames":request.frames, "blocks":request.frames/256, "latency_samples":plugin.latency_samples(),
        "native_adaptation":"fresh two-path PBFDAF, 50 ms, step_size 0.5", "feature_version":1,
        "feature_shape":[BANDS,12], "target_shape":[2,BANDS], "gain_floor":0.25,
        "features":features.finish()?, "targets":targets.finish()?, "error_spectra":error_spectra.finish()?,
        "classical":classical.finish()?, "adaptive":adaptive.finish()?}),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-aec-residual-corpus OUTPUT_DIRECTORY")?;
    let root = Path::new(&root);
    fs::create_dir_all(root)?;
    let mut stdout = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let line = line?;
        let result = match serde_json::from_str::<Request>(&line) {
            Ok(request) => export(root, &request).unwrap_or_else(
                |e| json!({"status":"error", "id":request.id, "error":e.to_string()}),
            ),
            Err(e) => json!({"status":"error", "error":e.to_string()}),
        };
        serde_json::to_writer(&mut stdout, &result)?;
        writeln!(stdout)?;
        stdout.flush()?;
    }
    Ok(())
}
