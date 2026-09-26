// ============================================================================
// Async ML Inference Thread for Vocal Detection
// ============================================================================
//
// Runs ONNX model inference through the shared `plugins-inference` bridge:
// the audio thread pushes feature frames without blocking, a worker thread
// runs the tract model, and the latest vocal probability is published behind
// a generation counter (a reset invalidates stale work).

use super::ml_features::{CONTEXT_FRAMES, FEATURE_SIZE, FRAME_FEATURE_SIZE};
use plugins_inference::{AsyncInference, InferenceModel};
use tract_onnx::prelude::*;

/// Optimised+runnable tract model. Type alias for the value returned by
/// `TypedModel::into_runnable()`; keeps function signatures readable.
type RunnableOnnxModel = TypedRunnableModel<TypedModel>;

/// Ring buffer capacity in contexts (blocks arrive every ~21ms at 2048/48k with 50% overlap).
const RING_BUFFER_CAPACITY: usize = 4;

/// A single feature context sent from audio thread to inference thread.
pub struct MfccFrame {
    pub features: [f32; FEATURE_SIZE],
}

/// tract adapter: runs the vocal-detection model on the bridge worker thread.
struct OnnxModelAdapter {
    model: RunnableOnnxModel,
}

impl InferenceModel for OnnxModelAdapter {
    type Input = MfccFrame;
    type Output = f32;

    fn run(&mut self, input: &Self::Input) -> Result<Self::Output, String> {
        let mut input_data = vec![0.0_f32; FEATURE_SIZE];
        input_data.copy_from_slice(&input.features);
        run_inference(&self.model, &input_data).map(|v_prob| v_prob.clamp(0.0, 1.0))
    }
}

/// Audio-thread side handle for the ML inference system.
///
/// Thin wrapper over the shared async-inference bridge; all methods are
/// non-blocking and safe for real-time use.
pub struct MlInferenceHandle {
    inner: AsyncInference<OnnxModelAdapter>,
}

impl MlInferenceHandle {
    /// Create a new inference handle, loading the ONNX model and spawning the worker thread.
    ///
    /// Returns `Err` if the model cannot be loaded.
    pub fn new(model_path: &str) -> Result<Self, String> {
        // Load the raw ONNX proto first so we can validate optional metadata
        // properties (informational; model still loads if absent).
        let proto = tract_onnx::onnx()
            .proto_model_for_path(model_path)
            .map_err(|e| format!("Failed to read ONNX model '{}': {}", model_path, e))?;
        validate_metadata_contract(&proto.metadata_props)?;

        // Build the runnable model from the proto we already parsed.
        let model = tract_onnx::onnx()
            .model_for_proto_model(&proto)
            .map_err(|e| format!("Failed to parse ONNX model '{}': {}", model_path, e))?
            .into_optimized()
            .map_err(|e| format!("Failed to optimise ONNX model: {}", e))?
            .into_runnable()
            .map_err(|e| format!("Failed to make ONNX model runnable: {}", e))?;

        validate_input_contract(&model)?;

        let inner = AsyncInference::spawn(
            OnnxModelAdapter { model },
            RING_BUFFER_CAPACITY,
            "ml-vocal-detect",
        )
        .map_err(|e| format!("Failed to spawn inference thread: {e}"))?;

        Ok(Self { inner })
    }

    /// Send feature context to the inference thread. Non-blocking.
    ///
    /// If the ring buffer is full, the frame is silently dropped (inference
    /// is slower than audio — the latest frame that fits will be used).
    #[inline]
    pub fn send_features(&mut self, features: &[f32; FEATURE_SIZE]) {
        self.inner.send(MfccFrame {
            features: *features,
        });
    }

    /// Read the latest V_prob from the inference thread. Non-blocking.
    ///
    /// Returns `None` until the first inference completes, then returns
    /// `Some(probability)` with the latest vocal detection probability.
    #[inline]
    pub fn read_v_prob(&self) -> Option<f32> {
        self.inner.latest()
    }

    /// Invalidate queued/in-flight features and clear the published result.
    pub fn reset(&self) {
        self.inner.reset();
    }
}

fn validate_input_contract(model: &RunnableOnnxModel) -> Result<(), String> {
    let fact = model
        .model()
        .input_fact(0)
        .map_err(|e| format!("ONNX model has no input #0: {}", e))?;

    if fact.datum_type != f32::datum_type() {
        return Err(format!(
            "ONNX model input must be f32, got {:?}",
            fact.datum_type
        ));
    }

    // Convert tract's symbolic shape to a concrete `Vec<i64>`-style view that
    // mirrors the original ort shape contract (dim or -1 wildcard).
    let dims: Vec<i64> = fact
        .shape
        .iter()
        .map(|d| d.to_i64().unwrap_or(-1))
        .collect();

    if !shape_accepts_feature_size(&dims) {
        return Err(format!(
            "ONNX model input must have shape [1, {}] or [-1, {}], got {:?}",
            FEATURE_SIZE, FEATURE_SIZE, dims
        ));
    }
    Ok(())
}

fn shape_accepts_feature_size(shape: &[i64]) -> bool {
    shape.len() == 2
        && (shape[0] == 1 || shape[0] == -1)
        && (shape[1] == FEATURE_SIZE as i64 || shape[1] == -1)
}

/// The model must return exactly one f32 probability in the canonical `[1, 1]`
/// shape. Other shapes are rejected rather than silently selecting an arbitrary
/// first element.
fn output_shape_accepts_probability(shape: &[usize]) -> bool {
    shape == [1, 1]
}

fn validate_metadata_contract(
    props: &[tract_onnx::pb::StringStringEntryProto],
) -> Result<(), String> {
    let lookup = |key: &str| -> Option<&str> {
        props
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.value.as_str())
    };

    for (key, expected) in [
        ("feature_size", FEATURE_SIZE),
        ("frame_feature_size", FRAME_FEATURE_SIZE),
        ("context_frames", CONTEXT_FRAMES),
    ] {
        let Some(value) = lookup(key) else {
            continue;
        };
        let parsed = value.parse::<usize>().map_err(|_| {
            format!(
                "ONNX metadata '{}' must be an integer, got '{}'",
                key, value
            )
        })?;
        if parsed != expected {
            return Err(format!(
                "ONNX metadata '{}' mismatch: model has {}, plugin expects {}",
                key, parsed, expected
            ));
        }
    }

    if let Some(threshold) = lookup("recommended_threshold") {
        log::info!("ML vocal detector recommended threshold: {}", threshold);
    }

    Ok(())
}

/// Run a single inference pass. Returns the vocal probability (0.0-1.0).
fn run_inference(model: &RunnableOnnxModel, input_data: &[f32]) -> Result<f32, String> {
    let input = tract_ndarray::Array2::from_shape_vec((1, FEATURE_SIZE), input_data.to_vec())
        .map_err(|e| format!("Failed to create input array: {}", e))?;

    let outputs = model
        .run(tvec!(Tensor::from(input).into()))
        .map_err(|e| format!("Inference error: {}", e))?;

    if outputs.len() != 1 {
        return Err(format!(
            "Inference model must return exactly one output tensor, got {}",
            outputs.len()
        ));
    }

    let view = outputs[0]
        .to_array_view::<f32>()
        .map_err(|e| format!("Failed to extract output tensor: {}", e))?;

    if !output_shape_accepts_probability(view.shape()) {
        return Err(format!(
            "Inference output must have shape [1, 1], got {:?}",
            view.shape()
        ));
    }

    view.iter()
        .next()
        .copied()
        .ok_or_else(|| "Empty output tensor".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mfcc_frame_size() {
        let frame = MfccFrame {
            features: [0.0; FEATURE_SIZE],
        };
        assert_eq!(frame.features.len(), FEATURE_SIZE);
    }

    #[test]
    fn test_shape_accepts_current_feature_contract() {
        assert!(shape_accepts_feature_size(&[1, FEATURE_SIZE as i64]));
        assert!(shape_accepts_feature_size(&[-1, FEATURE_SIZE as i64]));
        assert!(shape_accepts_feature_size(&[1, -1]));
        assert!(!shape_accepts_feature_size(&[1, 40]));
        assert!(!shape_accepts_feature_size(&[FEATURE_SIZE as i64]));
        assert!(!shape_accepts_feature_size(&[2, FEATURE_SIZE as i64]));
    }

    #[test]
    fn output_contract_accepts_only_single_probability_tensor() {
        assert!(output_shape_accepts_probability(&[1, 1]));
        assert!(!output_shape_accepts_probability(&[1]));
        assert!(!output_shape_accepts_probability(&[2, 1]));
        assert!(!output_shape_accepts_probability(&[1, 2]));
        assert!(!output_shape_accepts_probability(&[]));
    }

    #[test]
    fn test_inference_handle_with_nonexistent_model() {
        let result = MlInferenceHandle::new("/nonexistent/model.onnx");
        assert!(result.is_err());
    }

    #[test]
    fn test_inference_with_dummy_model() {
        // Find the dummy model relative to the workspace root
        let model_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_data/dummy_vocal_detector.onnx"
        );
        if !std::path::Path::new(model_path).exists() {
            eprintln!("Skipping test: dummy model not found at {}", model_path);
            return;
        }

        let mut handle = MlInferenceHandle::new(model_path).expect("Should load dummy model");

        // Send a feature frame
        let features = [0.0_f32; FEATURE_SIZE];
        handle.send_features(&features);

        // Wait for inference to complete (dummy model should be fast)
        let mut v_prob = None;
        for _ in 0..100 {
            v_prob = handle.read_v_prob();
            if v_prob.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let prob = v_prob.expect("Should have received inference result");
        // Dummy model outputs sigmoid(0) = 0.5
        assert!(
            (prob - 0.5).abs() < 0.01,
            "Dummy model should output ~0.5, got {}",
            prob
        );

        // Dropping the handle joins the worker thread.
        drop(handle);
    }

    #[test]
    fn test_fallback_when_no_model() {
        // When ML handle is None, read_v_prob should return None
        // This is tested implicitly through the detection.rs dispatch logic,
        // but we verify the handle behavior here
        let model_path = "/nonexistent/model.onnx";
        let result = MlInferenceHandle::new(model_path);
        assert!(result.is_err(), "Should fail for nonexistent model");
    }

    #[test]
    fn reset_during_streaming_recovers_with_fresh_result() {
        // Generation fencing moved into the shared bridge; here we pin the
        // observable contract: after hammering send/reset, a fresh input
        // still publishes and no stale pre-reset value surfaces.
        let model_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test_data/dummy_vocal_detector.onnx"
        );
        if !std::path::Path::new(model_path).exists() {
            eprintln!("Skipping test: dummy model not found at {}", model_path);
            return;
        }

        let mut handle = MlInferenceHandle::new(model_path).expect("Should load dummy model");
        for _ in 0..50 {
            handle.send_features(&[0.0; FEATURE_SIZE]);
            handle.reset();
        }
        handle.send_features(&[0.0; FEATURE_SIZE]);
        let mut v_prob = None;
        for _ in 0..100 {
            v_prob = handle.read_v_prob();
            if v_prob.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let prob = v_prob.expect("fresh input after resets must publish");
        assert!(
            (prob - 0.5).abs() < 0.01,
            "dummy model should output ~0.5, got {}",
            prob
        );
    }
}
