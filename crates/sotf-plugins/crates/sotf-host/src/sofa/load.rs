//! Control-side loading of physical SOFA impulse responses.

// Rust guideline compliant 2026-02-21
use super::{CoordinateSystem, SofaFile, SourcePosition};
use crate::fractional_delay::fractional_delay_coefficients;
use sofa_reader::{AttrValue, Hdf5File};
use std::path::Path;

// Bound amplification from tiny files carrying enormous delay values. This is a
// dataset capability limit for nonzero-delay materialization, independent of
// each renderer's per-IR support. Legacy raw/cache loading is unaffected.
const MAX_MATERIALIZED_IR_BYTES: usize = 256 * 1024 * 1024;

/// Loaded impulse responses and their applied delay metadata.
#[derive(Debug)]
pub struct LoadedSofa {
    /// Canonical HRIR samples, including supported SOFA delays exactly once.
    pub data: SofaFile,
    /// Whether nonzero `Data.Delay` values were materialized into the samples.
    pub delay_applied: bool,
    /// Dataset-wide causal offset added to each response, in source-rate samples.
    pub delay_rebase_samples: usize,
    /// Dataset-wide causal offset added to each response, in seconds.
    pub delay_rebase_seconds: f64,
}

/// Load physical HRIR samples and materialize supported SOFA `Data.Delay` values.
///
/// SOFA fields are parsed from one immutable byte buffer. Missing or all-zero
/// `Data.Delay` preserves the original samples and IR length. SQLite cache paths
/// retain the dependency loader: old caches contain no recoverable delay metadata.
/// Coordinate defaults and the legacy scalar sampling-rate attribute are preserved.
/// Work and allocations occur on the control thread, before filter publication.
///
/// # Errors
///
/// Returns an error for unreadable files, inconsistent FIR dimensions, nonuniform
/// or nonpositive/nonfinite rates, malformed delay shapes, or unrepresentable
/// storage (including a 256 MiB dataset limit for nonzero-delay materialization).
pub fn load_sofa(path: impl AsRef<Path>) -> Result<LoadedSofa, String> {
    let path = path.as_ref();
    if matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("hrtfdb" | "sqlite" | "db")
    ) {
        return SofaFile::load(path).map(|data| LoadedSofa {
            data,
            delay_applied: false,
            delay_rebase_samples: 0,
            delay_rebase_seconds: 0.0,
        });
    }
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Failed to read SOFA '{}': {e}", path.display()))?;
    let reader = Hdf5File::from_bytes(bytes).map_err(|e| format!("Invalid SOFA structure: {e}"))?;
    load_reader(&reader)
}

fn load_reader(reader: &Hdf5File) -> Result<LoadedSofa, String> {
    if !matches!(reader.attribute("Conventions"), Some(AttrValue::String(value)) if value == "SOFA")
    {
        log::warn!("[SOFA] Missing or non-SOFA Conventions global attribute");
    }
    let convention = reader
        .attribute_string("SOFAConventions")
        .map_err(|e| e.to_string())?;
    let measurements = reader.dimension("M").map_err(|e| e.to_string())?;
    let length = reader.dimension("N").map_err(|e| e.to_string())?;
    let receivers = reader.dimension("R").map_err(|e| e.to_string())?;
    if measurements == 0 || length == 0 || receivers != 2 {
        return Err(format!(
            "Unsupported SOFA FIR dimensions: M={measurements}, R={receivers}, N={length}; require nonempty measurements and two receivers"
        ));
    }
    let count = measurements
        .checked_mul(2)
        .and_then(|v| v.checked_mul(length))
        .ok_or_else(|| "SOFA FIR dimensions overflow".to_string())?;
    if reader.dataset_dims("Data.IR").map_err(|e| e.to_string())?
        != [measurements as u64, 2, length as u64]
    {
        return Err("Data.IR shape must be [M, R, N] with R=2".into());
    }
    let rate = sampling_rate(reader, measurements)?;
    let position_count = measurements
        .checked_mul(3)
        .ok_or_else(|| "SOFA position dimensions overflow".to_string())?;
    let position_data = reader
        .read_f32("SourcePosition")
        .map_err(|e| e.to_string())?;
    if position_data.len() != position_count {
        return Err(format!(
            "SourcePosition size mismatch: expected {position_count}, got {}",
            position_data.len()
        ));
    }
    let coordinates = match reader.attribute("SourcePosition:Type") {
        Some(AttrValue::String(value)) if value.contains("cartesian") => {
            CoordinateSystem::Cartesian
        }
        _ => CoordinateSystem::Spherical,
    };
    let positions = position_data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[a, b, c]| match coordinates {
            CoordinateSystem::Cartesian => SourcePosition::from_cartesian(a, b, c),
            CoordinateSystem::Spherical => SourcePosition::new(a, b, c),
        })
        .collect();
    let samples = reader.read_f32("Data.IR").map_err(|e| e.to_string())?;
    if samples.len() != count {
        return Err(format!(
            "Data.IR size mismatch: expected {count}, got {}",
            samples.len()
        ));
    }
    let delays = read_delays(reader, measurements)?;
    let delay_applied = delays.iter().any(|&delay| delay != 0.0);
    let (impulse_responses, ir_length, delay_rebase_samples) = if delay_applied {
        materialize_delays(samples, length, measurements, &delays)?
    } else {
        (samples, length, 0)
    };
    let delay_rebase_seconds = delay_rebase_samples as f64 / rate;
    Ok(LoadedSofa {
        data: SofaFile {
            sample_rate: rate,
            num_measurements: measurements,
            ir_length,
            positions,
            impulse_responses,
            convention,
            data_sample_rate: Some(rate),
        },
        delay_applied,
        delay_rebase_samples,
        delay_rebase_seconds,
    })
}

fn sampling_rate(reader: &Hdf5File, measurements: usize) -> Result<f64, String> {
    let values = if reader.has_dataset("Data.SamplingRate") {
        let shape = reader
            .dataset_dims("Data.SamplingRate")
            .map_err(|e| e.to_string())?;
        // A scalar dataset is accepted for compatibility with existing files.
        if !shape.is_empty() && shape != [1] && shape != [measurements as u64] {
            return Err("Data.SamplingRate shape must be scalar, [I], or [M]".into());
        }
        let values = reader
            .read_f64("Data.SamplingRate")
            .map_err(|e| e.to_string())?;
        let expected = shape.first().copied().unwrap_or(1) as usize;
        if values.len() != expected {
            return Err("Data.SamplingRate size does not match its shape".into());
        }
        values
    } else {
        vec![
            reader
                .attribute_f64("Data.SamplingRate")
                .map_err(|e| e.to_string())?,
        ]
    };
    let Some(&first) = values.first() else {
        return Err("Data.SamplingRate is empty".into());
    };
    if !first.is_finite() || first <= 0.0 {
        return Err("Data.SamplingRate must be positive and finite".into());
    }
    if values.iter().any(|&value| value != first) {
        return Err("Unsupported SOFA capability: nonuniform Data.SamplingRate".into());
    }
    Ok(first)
}

fn read_delays(reader: &Hdf5File, measurements: usize) -> Result<Vec<f64>, String> {
    if !reader.has_dataset("Data.Delay") {
        return Ok(Vec::new());
    }
    let shape = reader
        .dataset_dims("Data.Delay")
        .map_err(|e| e.to_string())?;
    if shape != [1, 2] && shape != [measurements as u64, 2] {
        return Err("Data.Delay shape must be [I, R] or [M, R] with I=1 and R=2".into());
    }
    let values = reader.read_f64("Data.Delay").map_err(|e| e.to_string())?;
    let expected = (shape[0] as usize)
        .checked_mul(2)
        .ok_or_else(|| "Data.Delay dimensions overflow".to_string())?;
    if values.len() != expected {
        return Err("Data.Delay size does not match its shape".into());
    }
    values
        .into_iter()
        .map(|value| {
            if !value.is_finite() {
                return Err("Data.Delay must contain finite sample counts".into());
            }
            Ok(value)
        })
        .collect()
}

fn materialize_delays(
    samples: Vec<f32>,
    length: usize,
    measurements: usize,
    delays: &[f64],
) -> Result<(Vec<f32>, usize, usize), String> {
    let has_fractional_delay = delays.iter().any(|delay| delay.fract() != 0.0);
    if !has_fractional_delay {
        let minimum = delays.iter().copied().fold(f64::INFINITY, f64::min);
        let offset = if minimum < 0.0 {
            checked_sample_count(-minimum, "SOFA common causal offset")?
        } else {
            0
        };
        let integer_delays = delays
            .iter()
            .map(|&delay| {
                let magnitude = checked_sample_count(delay.abs(), "Data.Delay")?;
                if delay < 0.0 {
                    offset.checked_sub(magnitude).ok_or_else(|| {
                        "SOFA causal offset does not cover every negative delay".to_string()
                    })
                } else {
                    offset
                        .checked_add(magnitude)
                        .ok_or_else(|| "Delayed SOFA effective sample count overflow".to_string())
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let maximum = integer_delays.iter().copied().max().unwrap_or(0);
        let (output, expanded) =
            materialize_integer_delays(samples, length, measurements, &integer_delays, maximum)?;
        return Ok((output, expanded, offset));
    }

    let minimum = delays.iter().copied().fold(f64::INFINITY, f64::min);
    // ceil(32 - d) is exactly 32 - floor(d). Keeping the integer and
    // fractional parts separate avoids rounding away tiny negative delays.
    let offset_value = (32.0 - minimum.floor()).max(0.0);
    let offset = checked_sample_count(offset_value, "SOFA common causal offset")?;
    // Above this magnitude, f64 cannot preserve the 32-sample center margin
    // consistently when the integer delay and causal offset are combined.
    if offset as f64 > 9_007_199_254_740_992.0 {
        return Err(
            "Unsupported SOFA capability: fractional delay offset exceeds exact sample arithmetic"
                .into(),
        );
    }

    let response_count = measurements
        .checked_mul(2)
        .ok_or_else(|| "SOFA FIR dimensions overflow".to_string())?;
    let mut expanded = length;
    for index in 0..response_count {
        let delay = delays[if delays.len() == 2 { index % 2 } else { index }];
        let fractional = delay.fract() != 0.0;
        let prefix = delay_prefix(delay, offset, fractional)?;
        let support = if fractional { 64 } else { 0 };
        let end = prefix
            .checked_add(length)
            .and_then(|value| value.checked_add(support))
            .ok_or_else(|| "Delayed SOFA IR length overflow".to_string())?;
        expanded = expanded.max(end);
    }
    let count = checked_materialized_count(measurements, expanded)?;
    let bytes = count
        .checked_mul(size_of::<f32>())
        .ok_or_else(|| "Delayed SOFA IR byte count overflow".to_string())?;
    if bytes > MAX_MATERIALIZED_IR_BYTES {
        return Err(format!(
            "Unsupported SOFA capability: delayed IR storage requests {bytes} bytes; limit is {MAX_MATERIALIZED_IR_BYTES} bytes"
        ));
    }

    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|e| format!("Unable to allocate delayed SOFA IRs: {e}"))?;
    output.resize(count, 0.0);
    for (index, ir) in samples.chunks_exact(length).enumerate() {
        let delay = delays[if delays.len() == 2 { index % 2 } else { index }];
        let fractional = delay.fract() != 0.0;
        let prefix = delay_prefix(delay, offset, fractional)?;
        let start = index * expanded + prefix;
        if fractional {
            let fraction = delay - delay.floor();
            let coefficients = fractional_delay_coefficients(fraction);
            for (sample_index, &sample) in ir.iter().enumerate() {
                for (tap, &coefficient) in coefficients.iter().enumerate() {
                    output[start + sample_index + tap] += sample * coefficient;
                }
            }
        } else {
            output[start..start + length].copy_from_slice(ir);
        }
    }
    Ok((output, expanded, offset))
}

fn delay_prefix(delay: f64, offset: usize, fractional: bool) -> Result<usize, String> {
    let integer_delay = delay.floor();
    let fractional_center = if fractional { 32.0 } else { 0.0 };
    checked_sample_count(
        integer_delay + offset as f64 - fractional_center,
        "Data.Delay effective prefix",
    )
}

fn checked_sample_count(value: f64, name: &str) -> Result<usize, String> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value >= usize::MAX as f64 {
        return Err(format!(
            "Unsupported SOFA capability: {name} exceeds addressable storage"
        ));
    }
    Ok(value as usize)
}

fn checked_materialized_count(measurements: usize, expanded: usize) -> Result<usize, String> {
    measurements
        .checked_mul(2)
        .and_then(|value| value.checked_mul(expanded))
        .filter(|&count| count <= isize::MAX as usize / size_of::<f32>())
        .ok_or_else(|| "Delayed SOFA IR storage overflow".to_string())
}

fn materialize_integer_delays(
    samples: Vec<f32>,
    length: usize,
    measurements: usize,
    delays: &[usize],
    maximum: usize,
) -> Result<(Vec<f32>, usize), String> {
    let expanded = length
        .checked_add(maximum)
        .ok_or_else(|| "Delayed SOFA IR length overflow".to_string())?;
    let count = measurements
        .checked_mul(2)
        .and_then(|v| v.checked_mul(expanded))
        .filter(|&count| count <= isize::MAX as usize / size_of::<f32>())
        .ok_or_else(|| "Delayed SOFA IR storage overflow".to_string())?;
    let bytes = count
        .checked_mul(size_of::<f32>())
        .ok_or_else(|| "Delayed SOFA IR byte count overflow".to_string())?;
    if bytes > MAX_MATERIALIZED_IR_BYTES {
        return Err(format!(
            "Unsupported SOFA capability: delayed IR storage requests {bytes} bytes; limit is {MAX_MATERIALIZED_IR_BYTES} bytes"
        ));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|e| format!("Unable to allocate delayed SOFA IRs: {e}"))?;
    output.resize(count, 0.0);
    for (index, ir) in samples.chunks_exact(length).enumerate() {
        let delay = delays[if delays.len() == 2 { index % 2 } else { index }];
        let start = index * expanded + delay;
        output[start..start + length].copy_from_slice(ir);
    }
    Ok((output, expanded))
}
