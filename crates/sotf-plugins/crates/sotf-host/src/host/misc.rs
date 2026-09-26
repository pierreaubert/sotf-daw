#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::external_plugin_ipc::{PluginSandboxBackendCode, PluginSandboxStatusCode};
use std::any::Any;

pub(super) const PARAMETER_EVENT_QUEUE_CAPACITY: usize = 1024;

pub(super) const GRAPH_MUTATION_QUEUE_CAPACITY: usize = 128;

pub(super) const DEFAULT_PARALLEL_NODE_COST: u32 = 1;

pub(super) const MODERATE_PARALLEL_NODE_COST: u32 = 4;

pub(super) const HEAVY_PARALLEL_NODE_COST: u32 = 16;

pub(super) const MIN_PARALLEL_STAGE_WORK_UNITS: usize = 32 * 128 * 2;

pub(super) fn panic_payload_description(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(super) fn sandbox_reason_text(
    status: PluginSandboxStatusCode,
    backend: PluginSandboxBackendCode,
    error: Option<&str>,
) -> Option<String> {
    if let Some(error) = error
        && error.to_ascii_lowercase().contains("sandbox")
    {
        return Some(error.to_string());
    }

    match status {
        PluginSandboxStatusCode::Unsupported => Some(match backend {
            PluginSandboxBackendCode::MacosProcessIsolation => {
                "macOS native sandbox backend is unavailable in this build; worker uses process isolation"
                    .to_string()
            }
            PluginSandboxBackendCode::MacosAppSandboxHelper => {
                "macOS App Sandbox helper reported unsupported at runtime".to_string()
            }
            PluginSandboxBackendCode::WindowsProcessIsolation => {
                "Windows native sandbox backend is unavailable in this build; worker uses process isolation"
                    .to_string()
            }
            PluginSandboxBackendCode::LinuxLandlock => {
                "Linux sandbox backend reported unsupported at runtime".to_string()
            }
            PluginSandboxBackendCode::Unknown => {
                "sandbox backend is unsupported on this platform".to_string()
            }
        }),
        PluginSandboxStatusCode::Disabled => Some("sandbox disabled by policy".to_string()),
        PluginSandboxStatusCode::Enforced | PluginSandboxStatusCode::Unknown => None,
    }
}

/// Worker-side detail for an `Unsupported` sandbox status, capped to a
/// UI-sized first line. The supervisor captures worker stderr, which is
/// where sandbox entry failures (e.g. the exact Landlock/seccomp denial)
/// are reported; other statuses ignore it.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(super) fn sandbox_unsupported_detail(
    status: PluginSandboxStatusCode,
    worker_stderr: Option<&str>,
) -> Option<String> {
    if status != PluginSandboxStatusCode::Unsupported {
        return None;
    }
    let first = worker_stderr?.lines().next()?.trim();
    if first.is_empty() {
        return None;
    }
    Some(first.chars().take(256).collect())
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod sandbox_reason_tests {
    use super::*;

    #[test]
    fn unsupported_backend_has_reason() {
        let reason = sandbox_reason_text(
            PluginSandboxStatusCode::Unsupported,
            PluginSandboxBackendCode::MacosProcessIsolation,
            None,
        );
        assert!(reason.is_some());
    }

    #[test]
    fn sandbox_error_text_is_prioritized() {
        let reason = sandbox_reason_text(
            PluginSandboxStatusCode::Unknown,
            PluginSandboxBackendCode::Unknown,
            Some("sandbox is required but not enforced"),
        );
        assert_eq!(
            reason.as_deref(),
            Some("sandbox is required but not enforced")
        );
    }

    #[test]
    fn unsupported_detail_prefers_first_worker_stderr_line() {
        let reason = sandbox_unsupported_detail(
            PluginSandboxStatusCode::Unsupported,
            Some("failed to enter pre-load worker sandbox: EPERM\nsecond line"),
        );
        assert_eq!(
            reason.as_deref(),
            Some("failed to enter pre-load worker sandbox: EPERM")
        );
    }

    #[test]
    fn unsupported_detail_ignores_other_statuses_and_blank_stderr() {
        assert_eq!(
            sandbox_unsupported_detail(
                PluginSandboxStatusCode::Enforced,
                Some("failed to enter pre-load worker sandbox: EPERM"),
            ),
            None
        );
        assert_eq!(
            sandbox_unsupported_detail(PluginSandboxStatusCode::Unsupported, None),
            None
        );
        assert_eq!(
            sandbox_unsupported_detail(
                PluginSandboxStatusCode::Unsupported,
                Some("   \n"),
            ),
            None
        );
        let long = format!("a{}\nsecond", "b".repeat(300));
        let capped = sandbox_unsupported_detail(PluginSandboxStatusCode::Unsupported, Some(&long))
            .expect("long line must truncate, not vanish");
        assert_eq!(capped.chars().count(), 256);
    }
}
