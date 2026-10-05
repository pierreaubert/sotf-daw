//! Checked loader for RNNoise-nu `.rnnn` version 1 text models.
//!
//! Parses the whitespace-separated weight format produced by the upstream
//! training dump (`dump_rnn.py`) and consumed by `rnn_reader.c`, validates
//! it strictly, transposes each weight matrix exactly once into the
//! neuron-major layout this fork computes over, and returns an owned
//! [`RnnModel`](crate::rnn::RnnModel) with no borrowed or static lifetime
//! beyond the builtin tables.
//!
//! Accepted models must match the fork graph exactly: six layers in fixed
//! order with fixed dimensions (42x24, 24x24, 90x48, 114x96, 96x22, 24x1).
//! Anything else is rejected before weight storage is allocated. Per-layer
//! activation codes come from the file (0 = Tanh, 1 = Sigmoid, 2 = Relu)
//! and unknown codes are rejected; unlike the C reader this loader never
//! silently substitutes Tanh for an unrecognized code.
//!
//! Callers run this off the audio callback during preparation: parsing
//! allocates exact-sized weight vectors and cloneable model state.

use crate::rnn::{Activation, DenseLayer, GruLayer, RnnModel};
use std::borrow::Cow;
use std::convert::TryFrom;
use std::fmt;

/// Exact first line of every accepted model file.
const RNNN_HEADER_V1: &str = "rnnoise-nu model file version 1";

/// Fork neuron cap mirrored from the C reader's dimension guard.
const MAX_LAYER_DIM: i32 = 128;

/// Total signed-byte weights of a fork-graph model (87503).
pub const RNNN_TOTAL_WEIGHTS: usize = 87_503;

/// Total whitespace tokens after the header line (18 header + weights).
pub const RNNN_TOTAL_TOKENS: usize = RNNN_TOTAL_WEIGHTS + 6 * 3;

/// One expected layer of the fixed fork graph.
struct LayerSpec {
    name: &'static str,
    gru: bool,
    inputs: usize,
    neurons: usize,
}

/// Fixed fork graph in file order.
///
/// Connectivity is enforced by exact dimension match and mirrors the
/// fork's `compute_rnn` assembly: the noise GRU input (90) packs dense
/// (24) + VAD state (24) + features (42), and the denoise GRU input
/// (114) packs VAD state (24) + noise state (48) + features (42) with
/// no dense contribution. These values are pinned against the builtin
/// `MODEL` by unit test, so loader and bundled graph cannot drift
/// apart silently.
const LAYERS: [LayerSpec; 6] = [
    LayerSpec {
        name: "input_dense",
        gru: false,
        inputs: 42,
        neurons: 24,
    },
    LayerSpec {
        name: "vad_gru",
        gru: true,
        inputs: 24,
        neurons: 24,
    },
    LayerSpec {
        name: "noise_gru",
        gru: true,
        inputs: 90,
        neurons: 48,
    },
    LayerSpec {
        name: "denoise_gru",
        gru: true,
        inputs: 114,
        neurons: 96,
    },
    LayerSpec {
        name: "denoise_output",
        gru: false,
        inputs: 96,
        neurons: 22,
    },
    LayerSpec {
        name: "vad_output",
        gru: false,
        inputs: 24,
        neurons: 1,
    },
];

/// Checked `.rnnn` load failure.
///
/// Every variant carries the layer/array/token position so a corrupt asset
/// can be diagnosed without re-running the upstream toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelLoadError {
    /// Input is not valid UTF-8 text.
    InvalidEncoding,
    /// No header line (empty input or missing newline).
    MissingHeader,
    /// First line is not exactly the v1 header.
    BadHeader { found: String },
    /// Fewer tokens than the declared graph requires.
    Truncated {
        layer: &'static str,
        array: &'static str,
        expected: usize,
        got: usize,
    },
    /// Tokens remain after the complete graph.
    TrailingData { extra: usize },
    /// Declared dimensions outside 1..=128.
    DimensionOutOfRange {
        layer: &'static str,
        nb_inputs: i32,
        nb_neurons: i32,
    },
    /// Declared dimensions do not match the fork graph.
    DimensionMismatch {
        layer: &'static str,
        nb_inputs: i32,
        nb_neurons: i32,
        expected_inputs: usize,
        expected_neurons: usize,
    },
    /// Activation code outside {0, 1, 2}.
    UnknownActivation { layer: &'static str, code: i32 },
    /// Token is not a plain `-?\d+` integer.
    InvalidToken {
        layer: &'static str,
        array: &'static str,
        index: usize,
        token: String,
    },
    /// Weight integer outside the signed-byte range.
    WeightOutOfRange {
        layer: &'static str,
        array: &'static str,
        index: usize,
        value: i32,
    },
    /// Externally built model disagrees with the fork graph.
    ///
    /// Returned by [`RnnState::from_model`](crate::rnn::RnnState::from_model)
    /// when a caller-constructed dimension, slice length, or top-level
    /// size does not match the fixed graph; the loader never produces
    /// this because it sets every field from the validated table.
    InconsistentModel {
        layer: &'static str,
        field: &'static str,
        expected: usize,
        got: usize,
    },
}

impl fmt::Display for ModelLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEncoding => write!(f, "model bytes are not valid UTF-8"),
            Self::MissingHeader => write!(f, "model is missing its header line"),
            Self::BadHeader { found } => write!(
                f,
                "bad model header {found:?}; expected exactly {RNNN_HEADER_V1:?}"
            ),
            Self::Truncated {
                layer,
                array,
                expected,
                got,
            } => write!(
                f,
                "model truncated in {layer}/{array}: got {got} of {expected} values"
            ),
            Self::TrailingData { extra } => {
                write!(f, "model has {extra} trailing tokens after the graph")
            }
            Self::DimensionOutOfRange {
                layer,
                nb_inputs,
                nb_neurons,
            } => write!(
                f,
                "{layer} dimensions {nb_inputs}x{nb_neurons} outside 1..={MAX_LAYER_DIM}"
            ),
            Self::DimensionMismatch {
                layer,
                nb_inputs,
                nb_neurons,
                expected_inputs,
                expected_neurons,
            } => write!(
                f,
                "{layer} dimensions {nb_inputs}x{nb_neurons} do not match fork graph {expected_inputs}x{expected_neurons}"
            ),
            Self::UnknownActivation { layer, code } => write!(
                f,
                "{layer} activation code {code} is not 0 (Tanh), 1 (Sigmoid) or 2 (Relu)"
            ),
            Self::InvalidToken {
                layer,
                array,
                index,
                token,
            } => write!(
                f,
                "{layer}/{array}[{index}] is not an integer: {token:?}"
            ),
            Self::WeightOutOfRange {
                layer,
                array,
                index,
                value,
            } => write!(
                f,
                "{layer}/{array}[{index}] value {value} is outside the signed-byte range"
            ),
            Self::InconsistentModel {
                layer,
                field,
                expected,
                got,
            } => write!(
                f,
                "{layer}/{field} is {got} but the fork graph requires {expected}"
            ),
        }
    }
}

impl std::error::Error for ModelLoadError {}

/// Truncates untrusted text for error context.
fn excerpt(token: &str) -> String {
    token.chars().take(24).collect()
}

/// Parses one strict integer token (`-?\d+`, 32-bit range).
fn parse_int_token(token: &str) -> Option<i32> {
    let digits = token.strip_prefix('-').unwrap_or(token);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse::<i32>().ok()
}

/// Streaming token reader with truncation accounting.
struct TokenReader<'a> {
    tokens: std::str::SplitWhitespace<'a>,
    layer: &'static str,
    array: &'static str,
    index: usize,
    expected: usize,
}

impl<'a> TokenReader<'a> {
    fn next_weight(&mut self) -> Result<i8, ModelLoadError> {
        let token = self.tokens.next().ok_or(ModelLoadError::Truncated {
            layer: self.layer,
            array: self.array,
            expected: self.expected,
            got: self.index,
        })?;
        let value = parse_int_token(token).ok_or(ModelLoadError::InvalidToken {
            layer: self.layer,
            array: self.array,
            index: self.index,
            token: excerpt(token),
        })?;
        let weight = i8::try_from(value).map_err(|_| ModelLoadError::WeightOutOfRange {
            layer: self.layer,
            array: self.array,
            index: self.index,
            value,
        })?;
        self.index += 1;
        Ok(weight)
    }
}

/// Reads one weight matrix in file (input-major) order.
///
/// The file stores `raw[j * n + i]` (input `j` outer, neuron `i` inner);
/// the fork computes over neuron-major rows, so each value lands at
/// `out[i * m + j]`. This is the only transpose; it runs here, once, off
/// the audio callback.
fn read_matrix(
    tokens: &mut std::str::SplitWhitespace<'_>,
    layer: &'static str,
    array: &'static str,
    m: usize,
    n: usize,
) -> Result<Vec<i8>, ModelLoadError> {
    let mut reader = TokenReader {
        tokens: tokens.clone(),
        layer,
        array,
        index: 0,
        expected: m * n,
    };
    let mut out = vec![0i8; m * n];
    for j in 0..m {
        for i in 0..n {
            out[i * m + j] = reader.next_weight()?;
        }
    }
    *tokens = reader.tokens;
    Ok(out)
}

/// Reads one bias vector verbatim (no reorder, matching the C reader).
fn read_bias(
    tokens: &mut std::str::SplitWhitespace<'_>,
    layer: &'static str,
    n: usize,
) -> Result<Vec<i8>, ModelLoadError> {
    let mut reader = TokenReader {
        tokens: tokens.clone(),
        layer,
        array: "bias",
        index: 0,
        expected: n,
    };
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(reader.next_weight()?);
    }
    *tokens = reader.tokens;
    Ok(out)
}

/// Parses and validates one layer header triple.
fn read_header(
    tokens: &mut std::str::SplitWhitespace<'_>,
    spec: &LayerSpec,
) -> Result<Activation, ModelLoadError> {
    let mut header = [0i32; 3];
    for (slot, value) in header.iter_mut().enumerate() {
        let token = tokens.next().ok_or(ModelLoadError::Truncated {
            layer: spec.name,
            array: "header",
            expected: 3,
            got: slot,
        })?;
        *value = parse_int_token(token).ok_or(ModelLoadError::InvalidToken {
            layer: spec.name,
            array: "header",
            index: slot,
            token: excerpt(token),
        })?;
    }
    let [nb_inputs, nb_neurons, code] = header;
    if nb_inputs < 1 || nb_neurons < 1 || nb_inputs > MAX_LAYER_DIM || nb_neurons > MAX_LAYER_DIM {
        return Err(ModelLoadError::DimensionOutOfRange {
            layer: spec.name,
            nb_inputs,
            nb_neurons,
        });
    }
    if nb_inputs as usize != spec.inputs || nb_neurons as usize != spec.neurons {
        return Err(ModelLoadError::DimensionMismatch {
            layer: spec.name,
            nb_inputs,
            nb_neurons,
            expected_inputs: spec.inputs,
            expected_neurons: spec.neurons,
        });
    }
    match code {
        0 => Ok(Activation::Tanh),
        1 => Ok(Activation::Sigmoid),
        2 => Ok(Activation::Relu),
        _ => Err(ModelLoadError::UnknownActivation {
            layer: spec.name,
            code,
        }),
    }
}

/// Parses a strict `.rnnn` v1 model into owned neuron-major weights.
///
/// # Errors
///
/// Returns [`ModelLoadError`] for any encoding, header, dimension,
/// activation, token, range, truncation, or trailing-data violation. No
/// partial model is produced; weight vectors are allocated only after their
/// layer dimensions validate.
pub fn parse_rnnn_model(bytes: &[u8]) -> Result<RnnModel, ModelLoadError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ModelLoadError::InvalidEncoding)?;
    let (header, rest) = text.split_once('\n').ok_or(ModelLoadError::MissingHeader)?;
    if header != RNNN_HEADER_V1 {
        return Err(ModelLoadError::BadHeader {
            found: excerpt(header),
        });
    }
    let mut tokens = rest.split_whitespace();

    // File order per layer is header, input weights, recurrent weights
    // (GRU only), then bias. Each array is read into a named local in
    // that exact sequence before the layer is constructed, so struct
    // field order can never define the parse order.
    let input_dense_activation = read_header(&mut tokens, &LAYERS[0])?;
    debug_assert!(!LAYERS[0].gru);
    let input_dense_weights = read_matrix(
        &mut tokens,
        LAYERS[0].name,
        "input_weights",
        LAYERS[0].inputs,
        LAYERS[0].neurons,
    )?;
    let input_dense_bias = read_bias(&mut tokens, LAYERS[0].name, LAYERS[0].neurons)?;
    let input_dense = DenseLayer {
        bias: Cow::Owned(input_dense_bias),
        input_weights: Cow::Owned(input_dense_weights),
        nb_inputs: LAYERS[0].inputs,
        nb_neurons: LAYERS[0].neurons,
        activation: input_dense_activation,
    };

    let vad_activation = read_header(&mut tokens, &LAYERS[1])?;
    debug_assert!(LAYERS[1].gru);
    let vad_wide = 3 * LAYERS[1].neurons;
    let vad_input = read_matrix(
        &mut tokens,
        LAYERS[1].name,
        "input_weights",
        LAYERS[1].inputs,
        vad_wide,
    )?;
    let vad_recurrent = read_matrix(
        &mut tokens,
        LAYERS[1].name,
        "recurrent_weights",
        LAYERS[1].neurons,
        vad_wide,
    )?;
    let vad_bias = read_bias(&mut tokens, LAYERS[1].name, vad_wide)?;
    let vad_gru = GruLayer {
        bias: Cow::Owned(vad_bias),
        input_weights: Cow::Owned(vad_input),
        recurrent_weights: Cow::Owned(vad_recurrent),
        nb_inputs: LAYERS[1].inputs,
        nb_neurons: LAYERS[1].neurons,
        activation: vad_activation,
    };

    let noise_activation = read_header(&mut tokens, &LAYERS[2])?;
    debug_assert!(LAYERS[2].gru);
    let noise_wide = 3 * LAYERS[2].neurons;
    let noise_input = read_matrix(
        &mut tokens,
        LAYERS[2].name,
        "input_weights",
        LAYERS[2].inputs,
        noise_wide,
    )?;
    let noise_recurrent = read_matrix(
        &mut tokens,
        LAYERS[2].name,
        "recurrent_weights",
        LAYERS[2].neurons,
        noise_wide,
    )?;
    let noise_bias = read_bias(&mut tokens, LAYERS[2].name, noise_wide)?;
    let noise_gru = GruLayer {
        bias: Cow::Owned(noise_bias),
        input_weights: Cow::Owned(noise_input),
        recurrent_weights: Cow::Owned(noise_recurrent),
        nb_inputs: LAYERS[2].inputs,
        nb_neurons: LAYERS[2].neurons,
        activation: noise_activation,
    };

    let denoise_activation = read_header(&mut tokens, &LAYERS[3])?;
    debug_assert!(LAYERS[3].gru);
    let denoise_wide = 3 * LAYERS[3].neurons;
    let denoise_input = read_matrix(
        &mut tokens,
        LAYERS[3].name,
        "input_weights",
        LAYERS[3].inputs,
        denoise_wide,
    )?;
    let denoise_recurrent = read_matrix(
        &mut tokens,
        LAYERS[3].name,
        "recurrent_weights",
        LAYERS[3].neurons,
        denoise_wide,
    )?;
    let denoise_bias = read_bias(&mut tokens, LAYERS[3].name, denoise_wide)?;
    let denoise_gru = GruLayer {
        bias: Cow::Owned(denoise_bias),
        input_weights: Cow::Owned(denoise_input),
        recurrent_weights: Cow::Owned(denoise_recurrent),
        nb_inputs: LAYERS[3].inputs,
        nb_neurons: LAYERS[3].neurons,
        activation: denoise_activation,
    };

    let denoise_out_activation = read_header(&mut tokens, &LAYERS[4])?;
    debug_assert!(!LAYERS[4].gru);
    let denoise_out_weights = read_matrix(
        &mut tokens,
        LAYERS[4].name,
        "input_weights",
        LAYERS[4].inputs,
        LAYERS[4].neurons,
    )?;
    let denoise_out_bias = read_bias(&mut tokens, LAYERS[4].name, LAYERS[4].neurons)?;
    let denoise_output = DenseLayer {
        bias: Cow::Owned(denoise_out_bias),
        input_weights: Cow::Owned(denoise_out_weights),
        nb_inputs: LAYERS[4].inputs,
        nb_neurons: LAYERS[4].neurons,
        activation: denoise_out_activation,
    };

    let vad_out_activation = read_header(&mut tokens, &LAYERS[5])?;
    debug_assert!(!LAYERS[5].gru);
    let vad_out_weights = read_matrix(
        &mut tokens,
        LAYERS[5].name,
        "input_weights",
        LAYERS[5].inputs,
        LAYERS[5].neurons,
    )?;
    let vad_out_bias = read_bias(&mut tokens, LAYERS[5].name, LAYERS[5].neurons)?;
    let vad_output = DenseLayer {
        bias: Cow::Owned(vad_out_bias),
        input_weights: Cow::Owned(vad_out_weights),
        nb_inputs: LAYERS[5].inputs,
        nb_neurons: LAYERS[5].neurons,
        activation: vad_out_activation,
    };

    if tokens.next().is_some() {
        return Err(ModelLoadError::TrailingData {
            extra: 1 + tokens.count(),
        });
    }

    // Pin the published totals against the parsed arrays so the table,
    // the constants, and this function cannot drift apart silently.
    let weight_count = input_dense.bias.len()
        + input_dense.input_weights.len()
        + vad_gru.bias.len()
        + vad_gru.input_weights.len()
        + vad_gru.recurrent_weights.len()
        + noise_gru.bias.len()
        + noise_gru.input_weights.len()
        + noise_gru.recurrent_weights.len()
        + denoise_gru.bias.len()
        + denoise_gru.input_weights.len()
        + denoise_gru.recurrent_weights.len()
        + denoise_output.bias.len()
        + denoise_output.input_weights.len()
        + vad_output.bias.len()
        + vad_output.input_weights.len();
    debug_assert_eq!(weight_count, RNNN_TOTAL_WEIGHTS);
    debug_assert_eq!(weight_count + LAYERS.len() * 3, RNNN_TOTAL_TOKENS);

    Ok(RnnModel {
        input_dense_size: LAYERS[0].neurons,
        input_dense,
        vad_gru_size: LAYERS[1].neurons,
        vad_gru,
        noise_gru_size: LAYERS[2].neurons,
        noise_gru,
        denoise_gru_size: LAYERS[3].neurons,
        denoise_gru,
        denoise_output_size: LAYERS[4].neurons,
        denoise_output,
        vad_output_size: LAYERS[5].neurons,
        vad_output,
    })
}

/// Rejects a caller-built model that disagrees with the fork graph.
///
/// Checks exact dimensions, exact slice lengths, and exact top-level
/// sizes for all six layers before any state is allocated, so an
/// inconsistent model fails here instead of overflowing the fixed
/// scratch arrays, panicking on indexing, or requesting an absurd
/// allocation in `from_model`. Dimension checks run before any length
/// arithmetic, so hostile values like `usize::MAX` cannot overflow.
pub(crate) fn check_fork_graph(model: &RnnModel) -> Result<(), ModelLoadError> {
    check_dense(&model.input_dense, model.input_dense_size, &LAYERS[0])?;
    check_gru(&model.vad_gru, model.vad_gru_size, &LAYERS[1])?;
    check_gru(&model.noise_gru, model.noise_gru_size, &LAYERS[2])?;
    check_gru(&model.denoise_gru, model.denoise_gru_size, &LAYERS[3])?;
    check_dense(&model.denoise_output, model.denoise_output_size, &LAYERS[4])?;
    check_dense(&model.vad_output, model.vad_output_size, &LAYERS[5])?;
    Ok(())
}

fn check_dense(layer: &DenseLayer, size: usize, spec: &LayerSpec) -> Result<(), ModelLoadError> {
    check_dims(spec, layer.nb_inputs, layer.nb_neurons)?;
    check_len(spec, "bias", layer.bias.len(), spec.neurons)?;
    check_len(
        spec,
        "input_weights",
        layer.input_weights.len(),
        spec.inputs * spec.neurons,
    )?;
    check_len(spec, "size", size, spec.neurons)?;
    Ok(())
}

fn check_gru(layer: &GruLayer, size: usize, spec: &LayerSpec) -> Result<(), ModelLoadError> {
    check_dims(spec, layer.nb_inputs, layer.nb_neurons)?;
    let wide = 3 * spec.neurons;
    check_len(spec, "bias", layer.bias.len(), wide)?;
    check_len(
        spec,
        "input_weights",
        layer.input_weights.len(),
        spec.inputs * wide,
    )?;
    check_len(
        spec,
        "recurrent_weights",
        layer.recurrent_weights.len(),
        spec.neurons * wide,
    )?;
    check_len(spec, "size", size, spec.neurons)?;
    Ok(())
}

fn check_dims(spec: &LayerSpec, nb_inputs: usize, nb_neurons: usize) -> Result<(), ModelLoadError> {
    if nb_inputs != spec.inputs {
        return Err(ModelLoadError::InconsistentModel {
            layer: spec.name,
            field: "nb_inputs",
            expected: spec.inputs,
            got: nb_inputs,
        });
    }
    if nb_neurons != spec.neurons {
        return Err(ModelLoadError::InconsistentModel {
            layer: spec.name,
            field: "nb_neurons",
            expected: spec.neurons,
            got: nb_neurons,
        });
    }
    Ok(())
}

fn check_len(
    spec: &LayerSpec,
    field: &'static str,
    got: usize,
    expected: usize,
) -> Result<(), ModelLoadError> {
    if got != expected {
        return Err(ModelLoadError::InconsistentModel {
            layer: spec.name,
            field,
            expected,
            got,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a full-size synthetic v1 buffer in memory.
    ///
    /// No weight literals: every value derives from its stream position, so
    /// the transpose oracle below checks indexing rather than fixtures.
    fn synthetic_buffer(activations: [i32; 6], weight: &dyn Fn(usize) -> i32) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut text = String::from("rnnoise-nu model file version 1\n");
        let mut position = 0;
        let push_values = |text: &mut String, count: usize, position: &mut usize| {
            for _ in 0..count {
                write!(text, "{} ", weight(*position)).unwrap();
                *position += 1;
            }
            text.push('\n');
        };
        for (layer, code) in LAYERS.iter().zip(activations.iter()) {
            writeln!(text, "{} {} {}", layer.inputs, layer.neurons, code).unwrap();
            if layer.gru {
                let wide = 3 * layer.neurons;
                push_values(&mut text, layer.inputs * wide, &mut position);
                push_values(&mut text, layer.neurons * wide, &mut position);
                push_values(&mut text, wide, &mut position);
            } else {
                push_values(&mut text, layer.inputs * layer.neurons, &mut position);
                push_values(&mut text, layer.neurons, &mut position);
            }
        }
        text.into_bytes()
    }

    /// Distinctive in-range pattern: full-period-ish linear walk mod 251.
    fn pattern(position: usize) -> i32 {
        ((position * 37) % 251) as i32 - 125
    }

    fn total_weights(model: &RnnModel) -> usize {
        model.input_dense.bias.len()
            + model.input_dense.input_weights.len()
            + model.vad_gru.bias.len()
            + model.vad_gru.input_weights.len()
            + model.vad_gru.recurrent_weights.len()
            + model.noise_gru.bias.len()
            + model.noise_gru.input_weights.len()
            + model.noise_gru.recurrent_weights.len()
            + model.denoise_gru.bias.len()
            + model.denoise_gru.input_weights.len()
            + model.denoise_gru.recurrent_weights.len()
            + model.denoise_output.bias.len()
            + model.denoise_output.input_weights.len()
            + model.vad_output.bias.len()
            + model.vad_output.input_weights.len()
    }

    #[test]
    fn valid_synthetic_model_parses_with_exact_graph() {
        let bytes = synthetic_buffer([0, 1, 2, 0, 1, 2], &pattern);
        let model = parse_rnnn_model(&bytes).unwrap();
        assert_eq!(total_weights(&model), RNNN_TOTAL_WEIGHTS);
        assert_eq!(model.input_dense_size, 24);
        assert_eq!(model.vad_gru_size, 24);
        assert_eq!(model.noise_gru_size, 48);
        assert_eq!(model.denoise_gru_size, 96);
        assert_eq!(model.denoise_output_size, 22);
        assert_eq!(model.vad_output_size, 1);
        assert_eq!(model.input_dense.activation, Activation::Tanh);
        assert_eq!(model.vad_gru.activation, Activation::Sigmoid);
        assert_eq!(model.noise_gru.activation, Activation::Relu);
        assert_eq!(model.denoise_gru.activation, Activation::Tanh);
        assert_eq!(model.denoise_output.activation, Activation::Sigmoid);
        assert_eq!(model.vad_output.activation, Activation::Relu);
        // Loaded storage is owned; nothing borrows the input buffer.
        for owned in [
            &model.input_dense.bias,
            &model.input_dense.input_weights,
            &model.vad_gru.bias,
            &model.vad_gru.input_weights,
            &model.vad_gru.recurrent_weights,
            &model.noise_gru.bias,
            &model.noise_gru.input_weights,
            &model.noise_gru.recurrent_weights,
            &model.denoise_gru.bias,
            &model.denoise_gru.input_weights,
            &model.denoise_gru.recurrent_weights,
            &model.denoise_output.bias,
            &model.denoise_output.input_weights,
            &model.vad_output.bias,
            &model.vad_output.input_weights,
        ] {
            assert!(matches!(owned, Cow::Owned(_)));
        }
    }

    /// Builds a full-size synthetic v1 buffer with disjoint weight/bias bands.
    ///
    /// Mirrors [`synthetic_buffer`] layout exactly; only the value source
    /// differs per array kind so an order swap fails loudly.
    fn synthetic_split_buffer(
        activations: [i32; 6],
        weight: &dyn Fn(usize) -> i32,
        bias: &dyn Fn(usize) -> i32,
    ) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut text = String::from("rnnoise-nu model file version 1\n");
        let mut weight_position = 0;
        let mut bias_position = 0;
        let push_values = |text: &mut String,
                           count: usize,
                           position: &mut usize,
                           value: &dyn Fn(usize) -> i32| {
            for _ in 0..count {
                write!(text, "{} ", value(*position)).unwrap();
                *position += 1;
            }
            text.push('\n');
        };
        for (layer, code) in LAYERS.iter().zip(activations.iter()) {
            writeln!(text, "{} {} {}", layer.inputs, layer.neurons, code).unwrap();
            if layer.gru {
                let wide = 3 * layer.neurons;
                push_values(&mut text, layer.inputs * wide, &mut weight_position, weight);
                push_values(
                    &mut text,
                    layer.neurons * wide,
                    &mut weight_position,
                    weight,
                );
                push_values(&mut text, wide, &mut bias_position, bias);
            } else {
                push_values(
                    &mut text,
                    layer.inputs * layer.neurons,
                    &mut weight_position,
                    weight,
                );
                push_values(&mut text, layer.neurons, &mut bias_position, bias);
            }
        }
        text.into_bytes()
    }

    /// Regression: weights must land in weight arrays, biases in biases.
    ///
    /// Weight tokens live in [-3, 3] and bias tokens in [100, 119]; the
    /// bands are disjoint, so a bias-first reader places weight values
    /// into the bias arrays and fails every assertion below. Exact
    /// in-array placement is covered by the transpose oracles; this test
    /// pins only the stream order per layer.
    #[test]
    fn arrays_keep_file_order_weights_before_bias() {
        fn weight(position: usize) -> i32 {
            (position % 7) as i32 - 3
        }
        fn bias(position: usize) -> i32 {
            100 + (position % 20) as i32
        }
        let bytes = synthetic_split_buffer([0, 1, 2, 0, 1, 2], &weight, &bias);
        let model = parse_rnnn_model(&bytes).unwrap();
        let mut position = 0;
        for parsed in [
            &model.input_dense.bias,
            &model.vad_gru.bias,
            &model.noise_gru.bias,
            &model.denoise_gru.bias,
            &model.denoise_output.bias,
            &model.vad_output.bias,
        ] {
            for value in parsed.iter() {
                assert_eq!(
                    *value as i32,
                    bias(position),
                    "bias stream position {position}"
                );
                position += 1;
            }
        }
        for parsed in [
            &model.input_dense.input_weights,
            &model.vad_gru.input_weights,
            &model.vad_gru.recurrent_weights,
            &model.noise_gru.input_weights,
            &model.noise_gru.recurrent_weights,
            &model.denoise_gru.input_weights,
            &model.denoise_gru.recurrent_weights,
            &model.denoise_output.input_weights,
            &model.vad_output.input_weights,
        ] {
            for value in parsed.iter() {
                assert!(
                    (-3..=3).contains(&(*value as i32)),
                    "weight band violation: {}",
                    *value as i32
                );
            }
        }
    }

    /// `from_model` rejects caller-built graphs without panicking.
    ///
    /// Each axis is mutated alone on a builtin clone; every rejection
    /// carries the layer/field position, and the oversized cases prove
    /// validation runs before any state allocation.
    #[test]
    fn from_model_rejects_inconsistent_graphs() {
        use crate::rnn::RnnState;
        assert!(RnnState::from_model(crate::model::MODEL.clone()).is_ok());
        let mut bad = crate::model::MODEL.clone();
        bad.input_dense.nb_inputs = 43;
        assert!(matches!(
            RnnState::from_model(bad),
            Err(ModelLoadError::InconsistentModel { .. })
        ));
        let mut bad = crate::model::MODEL.clone();
        bad.vad_gru.nb_neurons = 25;
        assert!(matches!(
            RnnState::from_model(bad),
            Err(ModelLoadError::InconsistentModel { .. })
        ));
        let mut bad = crate::model::MODEL.clone();
        bad.input_dense.bias = Cow::Owned(bad.input_dense.bias[..20].to_vec());
        assert!(matches!(
            RnnState::from_model(bad),
            Err(ModelLoadError::InconsistentModel { .. })
        ));
        let mut bad = crate::model::MODEL.clone();
        let mut long = bad.noise_gru.input_weights.to_vec();
        long.push(0);
        bad.noise_gru.input_weights = Cow::Owned(long);
        assert!(matches!(
            RnnState::from_model(bad),
            Err(ModelLoadError::InconsistentModel { .. })
        ));
        let mut bad = crate::model::MODEL.clone();
        bad.denoise_gru_size = 97;
        assert!(matches!(
            RnnState::from_model(bad),
            Err(ModelLoadError::InconsistentModel { .. })
        ));
        // Absurd dimensions fail the exact-match check before any
        // allocation or length arithmetic can overflow.
        let mut bad = crate::model::MODEL.clone();
        bad.denoise_output.nb_neurons = usize::MAX;
        assert!(matches!(
            RnnState::from_model(bad),
            Err(ModelLoadError::InconsistentModel {
                layer: "denoise_output",
                ..
            })
        ));
        let mut bad = crate::model::MODEL.clone();
        bad.vad_gru_size = usize::MAX;
        assert!(RnnState::from_model(bad).is_err());
    }

    fn check_matrix(tokens: &mut std::str::SplitWhitespace<'_>, parsed: &[i8], m: usize, n: usize) {
        for j in 0..m {
            for i in 0..n {
                let raw: i32 = tokens.next().unwrap().parse().unwrap();
                assert_eq!(parsed[i * m + j], raw as i8, "m={m} n={n} i={i} j={j}");
            }
        }
    }

    fn check_bias(tokens: &mut std::str::SplitWhitespace<'_>, parsed: &[i8]) {
        for (index, value) in parsed.iter().enumerate() {
            let raw: i32 = tokens.next().unwrap().parse().unwrap();
            assert_eq!(*value, raw as i8, "bias[{index}]");
        }
    }

    fn skip_header(tokens: &mut std::str::SplitWhitespace<'_>) {
        for _ in 0..3 {
            tokens.next().unwrap();
        }
    }

    #[test]
    fn transpose_matches_column_major_file_order_exactly() {
        let bytes = synthetic_buffer([2, 2, 2, 2, 2, 2], &pattern);
        let model = parse_rnnn_model(&bytes).unwrap();
        // Re-walk the file-order stream independently and compare every value.
        let text = std::str::from_utf8(&bytes).unwrap();
        let body = text.split_once('\n').unwrap().1;
        let mut tokens = body.split_whitespace();
        skip_header(&mut tokens);
        check_matrix(&mut tokens, &model.input_dense.input_weights, 42, 24);
        check_bias(&mut tokens, &model.input_dense.bias);
        skip_header(&mut tokens);
        check_matrix(&mut tokens, &model.vad_gru.input_weights, 24, 72);
        check_matrix(&mut tokens, &model.vad_gru.recurrent_weights, 24, 72);
        check_bias(&mut tokens, &model.vad_gru.bias);
        skip_header(&mut tokens);
        check_matrix(&mut tokens, &model.noise_gru.input_weights, 90, 144);
        check_matrix(&mut tokens, &model.noise_gru.recurrent_weights, 48, 144);
        check_bias(&mut tokens, &model.noise_gru.bias);
        skip_header(&mut tokens);
        check_matrix(&mut tokens, &model.denoise_gru.input_weights, 114, 288);
        check_matrix(&mut tokens, &model.denoise_gru.recurrent_weights, 96, 288);
        check_bias(&mut tokens, &model.denoise_gru.bias);
        skip_header(&mut tokens);
        check_matrix(&mut tokens, &model.denoise_output.input_weights, 96, 22);
        check_bias(&mut tokens, &model.denoise_output.bias);
        skip_header(&mut tokens);
        check_matrix(&mut tokens, &model.vad_output.input_weights, 24, 1);
        check_bias(&mut tokens, &model.vad_output.bias);
        assert!(tokens.next().is_none());
    }

    #[test]
    fn builtin_model_matches_loader_graph_table() {
        let bundled = &crate::model::MODEL;
        for (layer, spec) in [
            (&bundled.input_dense, &LAYERS[0]),
            (&bundled.denoise_output, &LAYERS[4]),
            (&bundled.vad_output, &LAYERS[5]),
        ] {
            assert_eq!(layer.nb_inputs, spec.inputs);
            assert_eq!(layer.nb_neurons, spec.neurons);
        }
        for (layer, spec) in [
            (&bundled.vad_gru, &LAYERS[1]),
            (&bundled.noise_gru, &LAYERS[2]),
            (&bundled.denoise_gru, &LAYERS[3]),
        ] {
            assert_eq!(layer.nb_inputs, spec.inputs);
            assert_eq!(layer.nb_neurons, spec.neurons);
        }
        assert_eq!(bundled.input_dense_size, 24);
        assert_eq!(bundled.vad_gru_size, 24);
        assert_eq!(bundled.noise_gru_size, 48);
        assert_eq!(bundled.denoise_gru_size, 96);
        assert_eq!(bundled.denoise_output_size, 22);
        assert_eq!(bundled.vad_output_size, 1);
        // Builtin storage stays borrowed: zero-cost, no duplication.
        assert!(matches!(bundled.input_dense.bias, Cow::Borrowed(_)));
        assert!(matches!(
            bundled.denoise_gru.recurrent_weights,
            Cow::Borrowed(_)
        ));
    }

    /// Splits a valid buffer into header line + tokens for mutation.
    fn valid_parts() -> (String, Vec<String>) {
        let bytes = synthetic_buffer([0, 0, 1, 0, 1, 1], &pattern);
        let text = String::from_utf8(bytes).unwrap();
        let (header, rest) = text.split_once('\n').unwrap();
        (
            header.to_string(),
            rest.split_whitespace().map(str::to_string).collect(),
        )
    }

    fn assemble(header: &str, tokens: &[String]) -> Vec<u8> {
        format!("{header}\n{}", tokens.join(" ")).into_bytes()
    }

    #[test]
    fn malformed_headers_are_rejected() {
        assert_eq!(
            parse_rnnn_model(b"").unwrap_err(),
            ModelLoadError::MissingHeader
        );
        assert_eq!(
            parse_rnnn_model(b"rnnoise-nu model file version 1").unwrap_err(),
            ModelLoadError::MissingHeader
        );
        assert!(matches!(
            parse_rnnn_model(b"rnnoise-nu model file version 2\n").unwrap_err(),
            ModelLoadError::BadHeader { .. }
        ));
        assert!(matches!(
            parse_rnnn_model(b"junk header\n0 0 0\n").unwrap_err(),
            ModelLoadError::BadHeader { .. }
        ));
        assert_eq!(
            parse_rnnn_model(&[0xff, 0xfe, 0x00]).unwrap_err(),
            ModelLoadError::InvalidEncoding
        );
    }

    #[test]
    fn malformed_dimensions_activations_and_tokens_are_rejected() {
        let (header, mut tokens) = valid_parts();
        // Token 0: first layer nb_inputs.
        tokens[0] = "0".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::DimensionOutOfRange { .. }
        ));
        tokens[0] = "129".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::DimensionOutOfRange { .. }
        ));
        tokens[0] = "43".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::DimensionMismatch { .. }
        ));
        tokens[0] = "42".to_string();
        // Token 2: first layer activation.
        tokens[2] = "3".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::UnknownActivation { .. }
        ));
        tokens[2] = "-1".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::UnknownActivation { .. }
        ));
        tokens[2] = "tanh".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::InvalidToken { .. }
        ));
        tokens[2] = "0".to_string();
        // Token 3: first weight value.
        tokens[3] = "12x".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::InvalidToken { .. }
        ));
        tokens[3] = "+5".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::InvalidToken { .. }
        ));
        tokens[3] = "-".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            // A bare minus survives whitespace splitting but is not an
            // integer: a wire-representable malformed token.
            ModelLoadError::InvalidToken { .. }
        ));
        let (header, mut tokens) = valid_parts();
        tokens[3] = "128".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::WeightOutOfRange { .. }
        ));
        tokens[3] = "-129".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::WeightOutOfRange { .. }
        ));
        tokens[3] = "99999999999999999999".to_string();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::InvalidToken { .. }
        ));
        // Boundary values are accepted.
        let (header, mut tokens) = valid_parts();
        tokens[3] = "127".to_string();
        tokens[4] = "-128".to_string();
        assert!(parse_rnnn_model(&assemble(&header, &tokens)).is_ok());
    }

    #[test]
    fn truncation_and_trailing_data_are_rejected() {
        let (header, mut tokens) = valid_parts();
        assert_eq!(tokens.len(), RNNN_TOTAL_TOKENS);
        tokens.pop();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::Truncated { .. }
        ));
        let (header, mut tokens) = valid_parts();
        tokens.truncate(100);
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::Truncated { .. }
        ));
        let (header, mut tokens) = valid_parts();
        tokens.push("0".to_string());
        assert_eq!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::TrailingData { extra: 1 }
        );
        let (header, mut tokens) = valid_parts();
        tokens.extend(["1".to_string(), "2".to_string(), "3".to_string()]);
        assert_eq!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            ModelLoadError::TrailingData { extra: 3 }
        );
    }

    #[test]
    fn error_display_is_total() {
        // Every variant renders without panicking (exercises Display).
        let cases = [
            ModelLoadError::InvalidEncoding,
            ModelLoadError::MissingHeader,
            ModelLoadError::BadHeader {
                found: "x".to_string(),
            },
            ModelLoadError::Truncated {
                layer: "l",
                array: "a",
                expected: 1,
                got: 0,
            },
            ModelLoadError::TrailingData { extra: 2 },
            ModelLoadError::DimensionOutOfRange {
                layer: "l",
                nb_inputs: 0,
                nb_neurons: 0,
            },
            ModelLoadError::DimensionMismatch {
                layer: "l",
                nb_inputs: 0,
                nb_neurons: 0,
                expected_inputs: 1,
                expected_neurons: 1,
            },
            ModelLoadError::UnknownActivation {
                layer: "l",
                code: 9,
            },
            ModelLoadError::InvalidToken {
                layer: "l",
                array: "a",
                index: 0,
                token: "t".to_string(),
            },
            ModelLoadError::WeightOutOfRange {
                layer: "l",
                array: "a",
                index: 0,
                value: 999,
            },
        ];
        for error in &cases {
            let text = format!("{error}");
            assert!(!text.is_empty());
            let _: &dyn std::error::Error = error;
        }
    }
}
