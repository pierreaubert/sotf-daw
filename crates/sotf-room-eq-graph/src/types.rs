use super::identity::identity_matrix_parameters;
use autoeq::roomeq::{ChannelDspChain, PluginConfigWrapper as DspPluginConfig};

pub(super) fn append_channel_dsp_graph_branch<'a, F, I>(
    add_node: &mut F,
    edges: &mut Vec<sotf_audio::engine::PluginGraphEdgeConfig>,
    start: usize,
    chain: Option<&ChannelDspChain>,
    plugins: I,
    channel_count: usize,
    channel_name: &str,
) -> usize
where
    F: FnMut(String, serde_json::Value, usize) -> usize,
    I: IntoIterator<Item = &'a DspPluginConfig>,
{
    let mut prev = start;
    if let Some(drivers) = chain.and_then(|chain| chain.drivers.as_ref())
        && !drivers.is_empty()
    {
        let label = format!("room_eq_driver_sum_{channel_name}");
        let driver_sum = add_node(
            "matrix".to_string(),
            identity_matrix_parameters(channel_count, &label),
            channel_count,
        );
        for driver in drivers {
            let mut driver_prev = start;
            for plugin in &driver.plugins {
                let node = add_node(
                    plugin.plugin_type.clone(),
                    plugin.parameters.clone(),
                    channel_count,
                );
                edges.push(sotf_audio::engine::PluginGraphEdgeConfig {
                    from_node: driver_prev,
                    to_node: node,
                });
                driver_prev = node;
            }
            edges.push(sotf_audio::engine::PluginGraphEdgeConfig {
                from_node: driver_prev,
                to_node: driver_sum,
            });
        }
        prev = driver_sum;
    }

    for plugin in plugins {
        let node = add_node(
            plugin.plugin_type.clone(),
            plugin.parameters.clone(),
            channel_count,
        );
        edges.push(sotf_audio::engine::PluginGraphEdgeConfig {
            from_node: prev,
            to_node: node,
        });
        prev = node;
    }
    prev
}

pub(super) fn is_route_replaced_global_plugin(plugin: &DspPluginConfig) -> bool {
    plugin.plugin_type == "matrix"
        && (plugin
            .parameters
            .get("label")
            .and_then(|value| value.as_str())
            == Some("home_cinema_bass_management")
            || plugin
                .parameters
                .get("metadata")
                .and_then(|metadata| metadata.get("purpose"))
                .and_then(|value| value.as_str())
                == Some("home_cinema_bass_management"))
}

pub(super) fn plugin_stage(plugin: &DspPluginConfig) -> Option<&str> {
    plugin
        .parameters
        .get("room_eq_stage")
        .and_then(|value| value.as_str())
}
