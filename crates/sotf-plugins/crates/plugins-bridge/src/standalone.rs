//! Processing preparation for native wrappers that run without `DawHost`.

// Rust guideline compliant 2026-02-21
use sotf_host::oversampling::AutoOversampledPlugin;
use sotf_host::plugin::Plugin;

/// Apply requested oversampling before initializing a standalone native plugin.
///
/// Call on the control thread after restoring structural settings. The returned
/// plugin still needs `initialize` at the host sample rate. `DawHost` already
/// performs this preparation for its own nodes.
///
/// # Errors
/// Rejects unsupported factors, unequal channel layouts, or overflowing storage
/// sizes. Allocation follows the host's negotiated maximum callback size.
pub fn prepare_standalone_plugin(
    plugin: Box<dyn Plugin>,
    max_frames: usize,
) -> Result<Box<dyn Plugin>, String> {
    let Some(factor) = plugin.preferred_oversampling() else {
        return Ok(plugin);
    };
    if !matches!(factor, 2 | 4) {
        return Err(format!("Unsupported native oversampling factor: {factor}"));
    }
    max_frames
        .checked_mul(factor as usize)
        .and_then(|frames| frames.checked_mul(plugin.input_channels()))
        .and_then(|samples| samples.checked_mul(std::mem::size_of::<f32>()))
        .filter(|bytes| *bytes <= isize::MAX as usize)
        .ok_or_else(|| "Native oversampling storage size overflow".to_string())?;
    Ok(Box::new(AutoOversampledPlugin::new_with_max_frames(
        plugin, factor, max_frames,
    )?))
}
