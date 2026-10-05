use autoeq::roomeq::DspChainOutput;

pub(super) fn sorted_channel_names(output: &DspChainOutput) -> Vec<String> {
    let mut names: Vec<_> = output.channels.keys().cloned().collect();
    names.sort();
    names
}

pub(super) fn single_channel_matrix_parameters(
    channel_count: usize,
    source_index: usize,
    destination_index: usize,
    gain: f64,
    label: String,
    metadata: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut matrix = vec![0.0_f32; channel_count * channel_count];
    if source_index < channel_count && destination_index < channel_count {
        matrix[destination_index * channel_count + source_index] = gain as f32;
    }
    let mut parameters = serde_json::json!({
        "label": label,
        "input_channels": channel_count,
        "output_channels": channel_count,
        "matrix": matrix,
    });
    if let Some(metadata) = metadata {
        parameters["metadata"] = metadata;
    }
    parameters
}
