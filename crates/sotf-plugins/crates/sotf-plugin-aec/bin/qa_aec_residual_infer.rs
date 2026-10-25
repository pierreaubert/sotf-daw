//! Compare explicit native model predictions against a pinned reference batch.

// Rust guideline compliant 2026-02-21
use sotf_host::{CountingAlloc, assert_no_allocs};
use sotf_plugin_aec::residual_features::{BANDS, FEATURES};
use sotf_plugin_aec::residual_model::ResidualMaskModel;
use std::io::Read;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: qa-aec-residual-infer MODEL SHA256 FEATURES_F32LE".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&args[0])?
        .take(262_145)
        .read_to_end(&mut bytes)?;
    let model = ResidualMaskModel::from_bytes(&bytes, args[1].to_str().ok_or("invalid SHA256")?)?;
    let mut features = Vec::new();
    std::fs::File::open(&args[2])?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut features)?;
    let frame_bytes = BANDS * FEATURES * 4;
    if features.len() > 16 * 1024 * 1024
        || features.is_empty()
        || !features.len().is_multiple_of(frame_bytes)
    {
        return Err("invalid bounded feature batch".into());
    }
    let mut predictions = Vec::with_capacity(features.len() / frame_bytes);
    for frame in features.chunks_exact(frame_bytes) {
        let mut values = [[0.0; FEATURES]; BANDS];
        for (value, bytes) in values
            .iter_mut()
            .flatten()
            .zip(frame.as_chunks::<4>().0.iter())
        {
            *value = f32::from_le_bytes(*bytes);
        }
        let mut result = None;
        assert_no_allocs("AEC residual model prediction", || {
            result = model.predict(&values);
        });
        predictions.push(result.ok_or("out-of-domain features")?);
    }
    println!("{}", serde_json::to_string(&predictions)?);
    Ok(())
}
