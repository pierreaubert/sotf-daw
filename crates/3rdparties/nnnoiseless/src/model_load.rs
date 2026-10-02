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
/// Connectivity is enforced by exact dimension match: the noise GRU input
/// packs dense (24) + VAD state (24) + features (42), and the denoise GRU
/// input packs dense (24) + VAD state (24) + noise state (48) + the last 18
/// features. These values are pinned against the builtin `MODEL` by unit
/// test, so loader and bundled graph cannot drift apart silently.
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
    BadHeader {
        found: String,
    },
    /// Fewer tokens than the declared graph requires.
    Truncated {
        layer: &'static str,
        array: &'static str,
        expected: usize,
        got: usize,
    },
    /// Tokens remain after the complete graph.
    TrailingData {
        extra: usize,
    },
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
    UnknownActivation {
        layer: &'static str,
        code: i32,
    },
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

    let input_dense_activation = read_header(&mut tokens, &LAYERS[0])?;
    let input_dense = DenseLayer {
        bias: Cow::Owned(read_bias(&mut tokens, LAYERS[0].name, LAYERS[0].neurons)?),
        input_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[0].name,
            "input_weights",
            LAYERS[0].inputs,
            LAYERS[0].neurons,
        )?),
        nb_inputs: LAYERS[0].inputs,
        nb_neurons: LAYERS[0].neurons,
        activation: input_dense_activation,
    };

    let vad_activation = read_header(&mut tokens, &LAYERS[1])?;
    let vad_gru = GruLayer {
        bias: Cow::Owned(read_bias(&mut tokens, LAYERS[1].name, 3 * LAYERS[1].neurons)?),
        input_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[1].name,
            "input_weights",
            LAYERS[1].inputs,
            3 * LAYERS[1].neurons,
        )?),
        recurrent_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[1].name,
            "recurrent_weights",
            LAYERS[1].neurons,
            3 * LAYERS[1].neurons,
        )?),
        nb_inputs: LAYERS[1].inputs,
        nb_neurons: LAYERS[1].neurons,
        activation: vad_activation,
    };

    let noise_activation = read_header(&mut tokens, &LAYERS[2])?;
    let noise_gru = GruLayer {
        bias: Cow::Owned(read_bias(&mut tokens, LAYERS[2].name, 3 * LAYERS[2].neurons)?),
        input_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[2].name,
            "input_weights",
            LAYERS[2].inputs,
            3 * LAYERS[2].neurons,
        )?),
        recurrent_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[2].name,
            "recurrent_weights",
            LAYERS[2].neurons,
            3 * LAYERS[2].neurons,
        )?),
        nb_inputs: LAYERS[2].inputs,
        nb_neurons: LAYERS[2].neurons,
        activation: noise_activation,
    };

    let denoise_activation = read_header(&mut tokens, &LAYERS[3])?;
    let denoise_gru = GruLayer {
        bias: Cow::Owned(read_bias(&mut tokens, LAYERS[3].name, 3 * LAYERS[3].neurons)?),
        input_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[3].name,
            "input_weights",
            LAYERS[3].inputs,
            3 * LAYERS[3].neurons,
        )?),
        recurrent_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[3].name,
            "recurrent_weights",
            LAYERS[3].neurons,
            3 * LAYERS[3].neurons,
        )?),
        nb_inputs: LAYERS[3].inputs,
        nb_neurons: LAYERS[3].neurons,
        activation: denoise_activation,
    };

    let denoise_out_activation = read_header(&mut tokens, &LAYERS[4])?;
    let denoise_output = DenseLayer {
        bias: Cow::Owned(read_bias(&mut tokens, LAYERS[4].name, LAYERS[4].neurons)?),
        input_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[4].name,
            "input_weights",
            LAYERS[4].inputs,
            LAYERS[4].neurons,
        )?),
        nb_inputs: LAYERS[4].inputs,
        nb_neurons: LAYERS[4].neurons,
        activation: denoise_out_activation,
    };

    let vad_out_activation = read_header(&mut tokens, &LAYERS[5])?;
    let vad_output = DenseLayer {
        bias: Cow::Owned(read_bias(&mut tokens, LAYERS[5].name, LAYERS[5].neurons)?),
        input_weights: Cow::Owned(read_matrix(
            &mut tokens,
            LAYERS[5].name,
            "input_weights",
            LAYERS[5].inputs,
            LAYERS[5].neurons,
        )?),
        nb_inputs: LAYERS[5].inputs,
        nb_neurons: LAYERS[5].neurons,
        activation: vad_out_activation,
    };

    if tokens.next().is_some() {
        return Err(ModelLoadError::TrailingData {
            extra: 1 + tokens.count(),
        });
    }

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
        let mut push_values = |text: &mut String, count: usize, position: &mut usize| {
            for _ in 0..count {
                write!(text, "{} ", weight(*position)).unwrap();
                *position += 1;
            }
            text.push('\n');
        };
        for (layer, code) in LAYERS.iter().zip(activations.iter()) {
            write!(text, "{} {} {}\n", layer.inputs, layer.neurons, code).unwrap();
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
        assert!(matches!(bundled.denoise_gru.recurrent_weights, Cow::Borrowed(_)));
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
        tokens[3] = String::new();
        assert!(matches!(
            parse_rnnn_model(&assemble(&header, &tokens)).unwrap_err(),
            // An empty token cannot survive whitespace splitting, so this
            // buffer is short one value: truncation, not a token error.
            ModelLoadError::Truncated { .. }
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
            ModelLoadError::UnknownActivation { layer: "l", code: 9 },
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
