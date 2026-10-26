//! Explicit QA-only ONNX Runtime binding and CPU-qualified CREPE execution.
//!
//! The application must initialize this binding before using other `ort` APIs.
//! The upstream loader and environment are process-global; this module binds one
//! canonical library path/hash for this process and refuses later substitutions.
//! Library/model loading and session creation are control-thread operations.

// Rust guideline compliant 2026-02-21
use crate::NeuralPitchConfig;
use ort::ep::coreml::{ComputeUnits, ModelFormat};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{TensorElementType, TensorRef, ValueType};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// CPU-qualified runtime choices; GPU-allowed CoreML failed activation parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrtPitchProvider {
    Cpu,
    CoreMlCpuOnly,
}

/// Explicit process-library identity and per-session QA provider selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrtPitchConfig {
    /// Absolute local runtime library path; never inferred or downloaded.
    pub library_path: PathBuf,
    /// SHA-256 of the selected runtime library bytes.
    pub library_sha256: String,
    /// Explicit CPU provider; CoreML registration failures are errors.
    pub provider: OrtPitchProvider,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LibraryIdentity {
    path: PathBuf,
    sha256: String,
}

// ort itself owns a process-global library/API/environment. A control-thread
// lock makes our identity check/initialization atomic across simultaneous loads.
static LIBRARY: Mutex<Option<LibraryIdentity>> = Mutex::new(None);

fn sha256_file(path: &Path) -> Result<String, String> {
    // Bound executable resources to 256 MiB without holding their bytes in RAM.
    const MAX_LIBRARY_BYTES: u64 = 256 * 1024 * 1024;
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_LIBRARY_BYTES {
        return Err("ORT library exceeds256MiB".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_LIBRARY_BYTES {
            return Err("ORT library exceeds256MiB".into());
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().iter().map(|v| format!("{v:02x}")).collect())
}

fn initialize(config: &OrtPitchConfig) -> Result<(), String> {
    if !config.library_path.is_absolute() || config.library_sha256.len() != 64 {
        return Err("ORT requires an absolute library path and SHA-256".into());
    }
    if config.provider == OrtPitchProvider::CoreMlCpuOnly && !cfg!(target_vendor = "apple") {
        return Err("CoreML pitch requires an Apple target".into());
    }
    let path = config
        .library_path
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let actual = sha256_file(&path)?;
    if actual != config.library_sha256 {
        return Err("ORT library SHA-256 mismatch".into());
    }
    let identity = LibraryIdentity {
        path,
        sha256: actual,
    };
    let mut binding = LIBRARY
        .lock()
        .map_err(|_| "ORT library binding lock poisoned")?;
    if let Some(current) = binding.as_ref() {
        return if *current == identity {
            Ok(())
        } else {
            Err("ORT process library is already bound to another path/hash".into())
        };
    }
    if !ort::init_from(&identity.path)
        .map_err(|e| e.to_string())?
        .commit()
    {
        return Err("ORT environment was initialized outside the explicit pitch binding".into());
    }
    *binding = Some(identity);
    Ok(())
}

#[derive(Debug)]
pub(crate) struct OrtPitchModel {
    session: Session,
    provider: OrtPitchProvider,
}

impl OrtPitchModel {
    pub(crate) fn load(
        config: &NeuralPitchConfig,
        runtime: &OrtPitchConfig,
    ) -> Result<Self, String> {
        Self::load_with_profile(config, runtime, None)
    }

    fn load_with_profile(
        config: &NeuralPitchConfig,
        runtime: &OrtPitchConfig,
        profile: Option<&Path>,
    ) -> Result<Self, String> {
        let mut bytes = Vec::new();
        File::open(&config.model_path)
            .map_err(|e| e.to_string())?
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let actual: String = Sha256::digest(&bytes)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        if bytes.len() > 64 * 1024 * 1024 || config.sha256 != actual {
            return Err("CREPE model size/SHA-256 mismatch".into());
        }
        initialize(runtime)?;
        let mut builder = Session::builder()
            .map_err(|e| e.to_string())?
            .with_intra_threads(1)
            .map_err(|e| e.to_string())?
            .with_inter_threads(1)
            .map_err(|e| e.to_string())?
            .with_parallel_execution(false)
            .map_err(|e| e.to_string())?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| e.to_string())?;
        let provider = match runtime.provider {
            OrtPitchProvider::Cpu => ort::ep::CPU::default()
                .with_arena_allocator(true)
                .build()
                .error_on_failure(),
            OrtPitchProvider::CoreMlCpuOnly => ort::ep::CoreML::default()
                .with_model_format(ModelFormat::NeuralNetwork)
                .with_compute_units(ComputeUnits::CPUOnly)
                .with_static_input_shapes(true)
                .build()
                .error_on_failure(),
        };
        builder = builder
            .with_execution_providers([provider])
            .map_err(|e| e.to_string())?;
        if let Some(path) = profile {
            builder = builder.with_profiling(path).map_err(|e| e.to_string())?;
        }
        let session = builder
            .commit_from_memory(&bytes)
            .map_err(|e| e.to_string())?;
        if session.inputs().len() != 1 || session.outputs().len() != 1 {
            return Err("CREPE requires one input/output".into());
        }
        for (outlet, expected) in [
            (session.inputs()[0].dtype(), &[1_i64, 1024][..]),
            (session.outputs()[0].dtype(), &[1_i64, 360][..]),
        ] {
            if !matches!(outlet, ValueType::Tensor { ty: TensorElementType::Float32, shape, .. } if &shape[..] == expected)
            {
                return Err("CREPE requires f32 [1,1024] -> [1,360]".into());
            }
        }
        let metadata = session.metadata().map_err(|e| e.to_string())?;
        for (key, expected) in [
            ("sotf_contract", "crepe-tiny-v1"),
            ("sample_rate", "16000"),
            ("frame_samples", "1024"),
            ("normalization", "zero-mean-sample-std-ddof1-min1e-10"),
        ] {
            if metadata.custom(key).as_deref() != Some(expected) {
                return Err(format!("CREPE metadata mismatch: {key}"));
            }
        }
        drop(metadata);
        Ok(Self {
            session,
            provider: runtime.provider,
        })
    }

    pub(crate) fn backend_name(&self) -> &'static str {
        match self.provider {
            OrtPitchProvider::Cpu => "ort_cpu",
            OrtPitchProvider::CoreMlCpuOnly => "ort_coreml_cpu_only",
        }
    }

    pub(crate) fn activations(&mut self, frame: &[f32; 1024]) -> Result<[f32; 360], String> {
        let input =
            TensorRef::from_array_view(([1_usize, 1024], &frame[..])).map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs![input])
            .map_err(|e| e.to_string())?;
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        if shape[..] != [1, 360]
            || data.len() != 360
            || data
                .iter()
                .any(|x| !x.is_finite() || !(0.0..=1.0).contains(x))
        {
            return Err("Invalid CREPE activation domain".into());
        }
        let mut result = [0.0; 360];
        result.copy_from_slice(data);
        Ok(result)
    }
}

/// Collect one native provider-placement profile without enabling worker profiling.
///
/// This prepares a separate session and writes its profile on the control thread.
///
/// # Errors
/// Rejects library/model/session failures or an unwritable profile prefix.
pub fn profile_execution(
    config: &NeuralPitchConfig,
    runtime: &OrtPitchConfig,
    prefix: &Path,
) -> Result<PathBuf, String> {
    let mut model = OrtPitchModel::load_with_profile(config, runtime, Some(prefix))?;
    model.activations(&[0.0; 1024])?;
    model
        .session
        .end_profiling()
        .map(PathBuf::from)
        .map_err(|e| e.to_string())
}
