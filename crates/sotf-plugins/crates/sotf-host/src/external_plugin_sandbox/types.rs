use super::plugin_sandbox_identity::PluginSandboxIdentity;
use super::plugin_sandbox_permission::PluginSandboxPermission;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PluginSandboxPermissionOutcome {
    Denied,
    Granted {
        grant: PluginSandboxUserGrant,
        persistence: PluginSandboxGrantPersistence,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PluginSandboxGrantPersistence {
    UntilRestart,
    RememberForPlugin,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginSandboxUserGrant {
    pub identity: PluginSandboxIdentity,
    pub permission: PluginSandboxPermission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PluginSandboxLifecycleMode {
    Import,
    AuthorizedRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PluginSandboxFileGrant {
    PluginBundleReadExecute,
    PresetDirectoryReadWrite { path: PathBuf },
    ReadOnlyPath { path: PathBuf },
    ReadWritePath { path: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PluginSandboxBrokerPolicy {
    NoPrompt,
    PromptAndRestart,
    ReportOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginSandboxBackendCapabilities {
    pub filesystem: bool,
    pub network: bool,
    pub local_authorization_profiles: bool,
    pub child_process_control: bool,
    pub prompt_without_restart: bool,
    pub store_compatible: bool,
}

#[cfg(target_os = "linux")]
pub(super) mod platform {
    use std::ffi::CString;
    use std::mem;
    use std::os::fd::RawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use crate::external_plugin::PluginDescriptor;

    use super::super::{ExternalPluginSandboxPolicy, ExternalPluginSandboxStatus};

    pub fn launch_backend() -> super::super::PluginSandboxLaunchBackend {
        super::super::PluginSandboxLaunchBackend::LinuxLandlockWorker
    }

    pub fn capabilities() -> super::super::PluginSandboxBackendCapabilities {
        launch_backend().capabilities()
    }

    const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
    const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

    const FS_EXECUTE: u64 = 1 << 0;
    const FS_WRITE_FILE: u64 = 1 << 1;
    const FS_READ_FILE: u64 = 1 << 2;
    const FS_READ_DIR: u64 = 1 << 3;
    const FS_REMOVE_DIR: u64 = 1 << 4;
    const FS_REMOVE_FILE: u64 = 1 << 5;
    const FS_MAKE_CHAR: u64 = 1 << 6;
    const FS_MAKE_DIR: u64 = 1 << 7;
    const FS_MAKE_REG: u64 = 1 << 8;
    const FS_MAKE_SOCK: u64 = 1 << 9;
    const FS_MAKE_FIFO: u64 = 1 << 10;
    const FS_MAKE_BLOCK: u64 = 1 << 11;
    const FS_MAKE_SYM: u64 = 1 << 12;
    const FS_REFER: u64 = 1 << 13;
    const FS_TRUNCATE: u64 = 1 << 14;

    const NET_BIND_TCP: u64 = 1 << 0;
    const NET_CONNECT_TCP: u64 = 1 << 1;

    #[repr(C)]
    struct LandlockRulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }

    #[repr(C)]
    struct LandlockPathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    pub fn enter(
        policy: &ExternalPluginSandboxPolicy,
        descriptor: &PluginDescriptor,
        shared_memory_path: &Path,
    ) -> Result<ExternalPluginSandboxStatus, String> {
        let abi = landlock_abi()?;
        if abi <= 0 {
            set_no_new_privs()?;
            return Ok(ExternalPluginSandboxStatus::Unsupported {
                backend: "linux-landlock",
                reason: "Landlock is not supported or disabled by the running kernel".to_string(),
            });
        }

        let handled_access_fs = fs_access_mask_for_abi(abi);
        let handled_access_net = if abi >= 4 && !policy.allow_network {
            NET_BIND_TCP | NET_CONNECT_TCP
        } else {
            0
        };
        let mut unsupported_reasons = Vec::new();
        if abi < 4 && !policy.allow_network {
            unsupported_reasons.push("network denial requires Landlock ABI 4 or newer".to_string());
        }
        // Installed after `restrict_self` below: the seccomp filter needs
        // `no_new_privs`, which `restrict_self` sets first.
        let deny_child_processes = !policy.allow_child_processes;
        let ruleset_attr = LandlockRulesetAttr {
            handled_access_fs,
            handled_access_net,
            scoped: 0,
        };

        let ruleset_fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &ruleset_attr,
                mem::size_of::<LandlockRulesetAttr>(),
                0,
            ) as RawFd
        };
        if ruleset_fd < 0 {
            return Err(format!(
                "failed to create Landlock ruleset: {}",
                std::io::Error::last_os_error()
            ));
        }

        let result = apply_rules(
            policy,
            descriptor,
            shared_memory_path,
            ruleset_fd,
            handled_access_fs,
        )
        .and_then(|_| restrict_self(ruleset_fd));
        unsafe {
            libc::close(ruleset_fd);
        }
        result?;

        if deny_child_processes
            && let Err(err) = super::super::seccomp_child_process::deny_child_processes()
        {
            unsupported_reasons.push(format!("child-process denial unavailable: {err}"));
        }

        if !unsupported_reasons.is_empty() {
            return Ok(ExternalPluginSandboxStatus::Unsupported {
                backend: "linux-landlock",
                reason: unsupported_reasons.join("; "),
            });
        }

        Ok(ExternalPluginSandboxStatus::Enforced {
            backend: "linux-landlock",
        })
    }

    /// Read/execute rights for a bundle or granted path. Landlock rejects
    /// directory-only rights on non-directories, so single-file bundles
    /// (`.clap`, `.so`) get no `FS_READ_DIR` while bundle directories
    /// (`.vst3`, `.component`) keep it.
    fn bundle_read_access(path: &Path) -> u64 {
        if canonicalize_if_possible(path).is_dir() {
            FS_READ_FILE | FS_READ_DIR | FS_EXECUTE
        } else {
            FS_READ_FILE | FS_EXECUTE
        }
    }

    fn apply_rules(
        policy: &ExternalPluginSandboxPolicy,
        descriptor: &PluginDescriptor,
        shared_memory_path: &Path,
        ruleset_fd: RawFd,
        handled_access_fs: u64,
    ) -> Result<(), String> {
        add_path_rule(
            ruleset_fd,
            &descriptor.path,
            bundle_read_access(&descriptor.path) & handled_access_fs,
        )?;

        add_path_rule(
            ruleset_fd,
            shared_memory_path,
            (FS_READ_FILE | FS_WRITE_FILE | FS_TRUNCATE) & handled_access_fs,
        )?;

        for path in &policy.extra_read_paths {
            add_path_rule(
                ruleset_fd,
                path,
                bundle_read_access(path) & handled_access_fs,
            )?;
        }
        for path in &policy.extra_write_paths {
            add_path_rule(ruleset_fd, path, writable_access() & handled_access_fs)?;
        }

        Ok(())
    }

    fn writable_access() -> u64 {
        FS_READ_FILE
            | FS_WRITE_FILE
            | FS_READ_DIR
            | FS_REMOVE_DIR
            | FS_REMOVE_FILE
            | FS_MAKE_CHAR
            | FS_MAKE_DIR
            | FS_MAKE_REG
            | FS_MAKE_SOCK
            | FS_MAKE_FIFO
            | FS_MAKE_BLOCK
            | FS_MAKE_SYM
            | FS_REFER
            | FS_TRUNCATE
    }

    fn fs_access_mask_for_abi(abi: i32) -> u64 {
        let mut mask = writable_access() | FS_EXECUTE;
        if abi < 2 {
            mask &= !FS_REFER;
        }
        if abi < 3 {
            mask &= !FS_TRUNCATE;
        }
        mask
    }

    fn add_path_rule(ruleset_fd: RawFd, path: &Path, access: u64) -> Result<(), String> {
        if access == 0 {
            return Ok(());
        }
        let path = canonicalize_if_possible(path);
        let fd = open_path_fd(&path)?;
        let rule = LandlockPathBeneathAttr {
            allowed_access: access,
            parent_fd: fd,
        };
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset_fd,
                LANDLOCK_RULE_PATH_BENEATH,
                &rule,
                0,
            )
        };
        unsafe {
            libc::close(fd);
        }
        if result < 0 {
            return Err(format!(
                "failed to add Landlock path rule for '{}': {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn canonicalize_if_possible(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    fn open_path_fd(path: &Path) -> Result<RawFd, String> {
        let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            format!(
                "cannot add sandbox path with interior NUL byte: '{}'",
                path.display()
            )
        })?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(format!(
                "failed to open sandbox path '{}': {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(fd)
    }

    fn restrict_self(ruleset_fd: RawFd) -> Result<(), String> {
        set_no_new_privs()?;
        let result = unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0) };
        if result < 0 {
            return Err(format!(
                "failed to enter Landlock sandbox: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn landlock_abi() -> Result<i32, String> {
        let abi = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<LandlockRulesetAttr>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        if abi < 0 {
            let err = std::io::Error::last_os_error();
            let code = err.raw_os_error().unwrap_or_default();
            if code == libc::ENOSYS || code == libc::EOPNOTSUPP {
                return Ok(0);
            }
            return Err(format!("failed to query Landlock ABI: {err}"));
        }
        Ok(abi as i32)
    }

    fn set_no_new_privs() -> Result<(), String> {
        let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if result != 0 {
            return Err(format!(
                "failed to set no_new_privs before sandboxing external plugin: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::mem::size_of;

        #[test]
        fn ruleset_attr_layout_matches_kernel_abi() {
            // Passed by reference to landlock_create_ruleset; any padding or
            // field drift silently corrupts enforcement, so pin the layout.
            assert_eq!(size_of::<LandlockRulesetAttr>(), 3 * size_of::<u64>());
            assert_eq!(size_of::<LandlockPathBeneathAttr>(), 2 * size_of::<u64>());
        }

        #[test]
        fn access_mask_tracks_landlock_abi() {
            let full = writable_access() | FS_EXECUTE;
            // ABI 1: no REFER, no TRUNCATE.
            assert_eq!(
                fs_access_mask_for_abi(1),
                full & !FS_REFER & !FS_TRUNCATE
            );
            // ABI 2: REFER added, TRUNCATE still absent.
            assert_eq!(fs_access_mask_for_abi(2), full & !FS_TRUNCATE);
            // ABI 3+: full filesystem mask; ABI 4 additionally gates TCP.
            assert_eq!(fs_access_mask_for_abi(3), full);
            assert_eq!(fs_access_mask_for_abi(4), full);
            assert_eq!(fs_access_mask_for_abi(99), full);
        }

        #[test]
        fn abi_probe_is_consistent() {
            // Probing never changes kernel state; a real syscall here keeps
            // the reported ABI honest instead of trusting the constants.
            let abi = landlock_abi().expect("ABI probe must not fail outright");
            assert!(
                abi >= 0,
                "negative ABI other than unsupported is a probe bug"
            );
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) mod platform {
    #[cfg(target_os = "macos")]
    const BACKEND_NAME: &str = "macos-process-isolation";
    #[cfg(target_os = "macos")]
    const MACOS_HELPER_BACKEND_NAME: &str = "macos-app-sandbox-helper";
    #[cfg(target_os = "macos")]
    const MACOS_APP_SANDBOX_CONTAINER_ENV: &str = "APP_SANDBOX_CONTAINER_ID";
    #[cfg(target_os = "windows")]
    const BACKEND_NAME: &str = "windows-process-isolation";
    #[cfg(target_os = "windows")]
    pub(super) const WINDOWS_APPCONTAINER_BACKEND_NAME: &str = "windows-appcontainer-worker";
    #[cfg(target_os = "macos")]
    const BACKEND_NOTE: &str =
        "macOS native sandbox backend is unavailable in this build; worker uses process isolation";
    #[cfg(target_os = "windows")]
    const BACKEND_NOTE: &str = "worker is not running inside the Windows AppContainer; process isolation only";

    #[cfg(target_os = "macos")]
    use std::ffi::OsStr;
    use std::fs::OpenOptions;
    use std::path::Path;

    use crate::external_plugin::PluginDescriptor;
    use crate::external_plugin_ipc;

    use super::super::{ExternalPluginSandboxPolicy, ExternalPluginSandboxStatus};

    #[cfg(target_os = "macos")]
    pub(in super::super) fn macos_launch_backend_from_container_id(
        container_id: Option<&OsStr>,
    ) -> super::super::PluginSandboxLaunchBackend {
        if container_id
            .and_then(OsStr::to_str)
            .is_some_and(|id| !id.is_empty())
        {
            return super::super::PluginSandboxLaunchBackend::MacosAppSandboxHelper;
        }

        super::super::PluginSandboxLaunchBackend::ProcessIsolationOnly {
            platform: BACKEND_NAME,
        }
    }

    pub fn launch_backend() -> super::super::PluginSandboxLaunchBackend {
        #[cfg(target_os = "macos")]
        {
            macos_launch_backend_from_container_id(
                std::env::var_os(MACOS_APP_SANDBOX_CONTAINER_ENV).as_deref(),
            )
        }

        #[cfg(target_os = "windows")]
        {
            super::super::PluginSandboxLaunchBackend::WindowsAppContainerWorker
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            super::super::PluginSandboxLaunchBackend::ProcessIsolationOnly {
                platform: BACKEND_NAME,
            }
        }
    }

    pub fn capabilities() -> super::super::PluginSandboxBackendCapabilities {
        launch_backend().capabilities()
    }

    pub fn enter(
        _policy: &ExternalPluginSandboxPolicy,
        _descriptor: &PluginDescriptor,
        _shared_memory_path: &Path,
    ) -> Result<ExternalPluginSandboxStatus, String> {
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        let file = options
            .open(_shared_memory_path)
            .map_err(|err| format!("shared memory is not accessible: {err}"))?;

        external_plugin_ipc::validate_shared_memory_file(&file, _shared_memory_path)
            .map_err(|err| format!("shared memory failed sandbox integrity check: {err}"))?;

        #[cfg(target_os = "macos")]
        if std::env::var_os(super::super::MACOS_APP_SANDBOX_HELPER_ENV).is_some() {
            return Ok(ExternalPluginSandboxStatus::Enforced {
                backend: MACOS_HELPER_BACKEND_NAME,
            });
        }

        #[cfg(target_os = "windows")]
        {
            enter_windows_appcontainer()
        }

        #[cfg(not(target_os = "windows"))]
        Ok(ExternalPluginSandboxStatus::Unsupported {
            backend: BACKEND_NAME,
            reason: BACKEND_NOTE.to_string(),
        })
    }

    /// Confirm the worker is actually confined in the rappct AppContainer.
    ///
    /// The token query is authoritative: unlike an environment flag it cannot
    /// be set by the plugin itself, so a worker launched without the
    /// `sotf-windows-sandbox-launcher` reports `Unsupported` and fails closed
    /// when the policy requires platform enforcement.
    #[cfg(target_os = "windows")]
    fn enter_windows_appcontainer() -> Result<ExternalPluginSandboxStatus, String> {
        match rappct::token::query_current_process_token() {
            Ok(info) if info.is_appcontainer => {
                Ok(ExternalPluginSandboxStatus::Enforced {
                    backend: WINDOWS_APPCONTAINER_BACKEND_NAME,
                })
            }
            Ok(_) => Ok(ExternalPluginSandboxStatus::Unsupported {
                backend: BACKEND_NAME,
                reason: BACKEND_NOTE.to_string(),
            }),
            Err(err) => Ok(ExternalPluginSandboxStatus::Unsupported {
                backend: BACKEND_NAME,
                reason: format!("Windows AppContainer token query failed: {err}"),
            }),
        }
    }
}
