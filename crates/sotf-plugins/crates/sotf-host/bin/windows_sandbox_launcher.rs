//! Host-owned launcher that runs the external plugin worker inside a Windows
//! AppContainer via rappct.
//!
//! The engine supervisor spawns this binary (see
//! `default_plugin_sandbox_launcher_command_for_backend`); it grants the
//! container package SID access to exactly the worker binary, plugin bundle,
//! policy paths, and IPC segment, then launches the worker confined and waits
//! for it. Worker output is discarded like the macOS helper; launcher errors
//! go to stderr, which the supervisor captures.

/// Shared parsing/planning logic. Compiled on Windows and for tests so the
/// argument handling and grant planning stay covered on every host.
#[cfg(any(target_os = "windows", test))]
mod common {
    use std::path::{Path, PathBuf};

    /// One planned filesystem grant for the container package SID.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FileGrant {
        pub path: PathBuf,
        pub directory: bool,
        pub writable: bool,
    }

    /// Parsed launcher invocation: worker identity plus args forwarded to the worker.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct LauncherArgs {
        pub worker_binary: PathBuf,
        pub worker_args: Vec<String>,
        pub worker_env: Vec<(String, String)>,
        pub forwarded_args: Vec<String>,
        pub descriptor_json: Option<String>,
        pub descriptor_file: Option<PathBuf>,
        pub sandbox_policy_json: Option<String>,
        pub allow_network: bool,
    }

    impl LauncherArgs {
        pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
            let mut worker_binary = None;
            let mut worker_args = Vec::new();
            let mut worker_env = Vec::new();
            let mut forwarded_args: Vec<String> = Vec::new();
            let mut iter = args.into_iter();

            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--sandbox-worker-binary" => {
                        worker_binary = Some(PathBuf::from(next_value(
                            "--sandbox-worker-binary",
                            &mut iter,
                        )?));
                    }
                    "--sandbox-worker-arg" => {
                        worker_args.push(next_value("--sandbox-worker-arg", &mut iter)?);
                    }
                    "--sandbox-worker-env" => {
                        let entry = next_value("--sandbox-worker-env", &mut iter)?;
                        let (key, value) = entry
                            .split_once('=')
                            .ok_or_else(|| "--sandbox-worker-env must be KEY=VALUE".to_string())?;
                        if key.is_empty() || key.contains('=') {
                            return Err("--sandbox-worker-env key must be non-empty".to_string());
                        }
                        worker_env.push((key.to_string(), value.to_string()));
                    }
                    _ => forwarded_args.push(arg),
                }
            }

            let worker_binary =
                worker_binary.ok_or_else(|| "missing --sandbox-worker-binary".to_string())?;
            let (descriptor_json, descriptor_file, sandbox_policy_json, allow_network) =
                skim_forwarded(&forwarded_args)?;
            Ok(Self {
                worker_binary,
                worker_args,
                worker_env,
                forwarded_args,
                descriptor_json,
                descriptor_file,
                sandbox_policy_json,
                allow_network,
            })
        }
    }

    /// Flags the launcher consumes itself; everything else still forwards.
    type SkimmedForwarded = (
        Option<String>,
        Option<PathBuf>,
        Option<String>,
        bool,
    );

    /// Pull launcher-relevant flags out of the forwarded worker args without
    /// removing them: the worker still needs every flag it was given.
    fn skim_forwarded(forwarded: &[String]) -> Result<SkimmedForwarded, String> {
        let mut descriptor_json = None;
        let mut descriptor_file = None;
        let mut sandbox_policy_json = None;
        let mut allow_network = false;
        let mut iter = forwarded.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--descriptor-json" => {
                    descriptor_json = Some(next_ref_value("--descriptor-json", &mut iter)?);
                }
                "--descriptor-file" => {
                    descriptor_file = Some(PathBuf::from(next_ref_value(
                        "--descriptor-file",
                        &mut iter,
                    )?));
                }
                "--sandbox-policy-json" => {
                    sandbox_policy_json = Some(next_ref_value("--sandbox-policy-json", &mut iter)?);
                }
                "--sandbox-allow-network" => allow_network = true,
                _ => {}
            }
        }
        Ok((
            descriptor_json,
            descriptor_file,
            sandbox_policy_json,
            allow_network,
        ))
    }

    fn next_value(
        flag: &'static str,
        iter: &mut impl Iterator<Item = String>,
    ) -> Result<String, String> {
        iter.next()
            .ok_or_else(|| format!("missing value for {flag}"))
    }

    fn next_ref_value<'a>(
        flag: &'static str,
        iter: &mut impl Iterator<Item = &'a String>,
    ) -> Result<String, String> {
        iter.next()
            .cloned()
            .ok_or_else(|| format!("missing value for {flag}"))
    }

    /// AppContainer profile name derived from the plugin id. Profile names
    /// accept a narrow charset, so anything else becomes `_`; empty ids fall back.
    pub fn profile_name_for_plugin(plugin_id: &str) -> String {
        let mut sanitized = String::with_capacity(plugin_id.len());
        for ch in plugin_id.chars() {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                sanitized.push(ch);
            } else {
                sanitized.push('_');
            }
        }
        let sanitized = sanitized.trim_matches(['_', '.']);
        let base = if sanitized.is_empty() {
            "external-plugin"
        } else {
            sanitized
        };
        let base: String = base.chars().take(48).collect();
        format!("sotf.plugin.{base}")
    }

    /// Quote one command-line argument using Windows `CommandLineToArgvW` rules.
    pub fn append_quoted(cmdline: &mut String, arg: &str) {
        if !arg.is_empty() && !arg.chars().any(|ch| ch == ' ' || ch == '\t' || ch == '"') {
            cmdline.push_str(arg);
            return;
        }
        cmdline.push('"');
        let mut backslashes = 0;
        for ch in arg.chars() {
            if ch == '\\' {
                backslashes += 1;
            } else if ch == '"' {
                for _ in 0..backslashes * 2 + 1 {
                    cmdline.push('\\');
                }
                cmdline.push('"');
                backslashes = 0;
            } else {
                for _ in 0..backslashes {
                    cmdline.push('\\');
                }
                cmdline.push(ch);
                backslashes = 0;
            }
        }
        for _ in 0..backslashes * 2 {
            cmdline.push('\\');
        }
        cmdline.push('"');
    }

    /// Parent directories needing a traverse grant so the container can reach
    /// `path`. Pure and platform-independent for testing.
    pub fn ancestor_dirs(path: &Path) -> Vec<PathBuf> {
        let mut ancestors = Vec::new();
        let mut current = path.parent();
        while let Some(dir) = current {
            // Stop at filesystem roots (`C:\`, `/`): the container always
            // traverses those via the logon session, and granting them is noise.
            if dir.parent().is_none() {
                break;
            }
            ancestors.push(dir.to_path_buf());
            current = dir.parent();
        }
        ancestors
    }

    pub fn shared_memory_from_forwarded(forwarded: &[String]) -> Option<PathBuf> {
        forwarded
            .windows(2)
            .find(|pair| pair[0] == "--shared-memory")
            .map(|pair| PathBuf::from(&pair[1]))
    }

    pub fn state_file_from_forwarded(forwarded: &[String]) -> Option<PathBuf> {
        forwarded
            .windows(2)
            .find(|pair| pair[0] == "--external-state-file")
            .map(|pair| PathBuf::from(&pair[1]))
    }

    pub fn plan_file_grants(
        worker_binary: &Path,
        descriptor_path: &Path,
        args: &LauncherArgs,
        policy: Option<&sotf_host::PluginSandboxPolicy>,
        is_dir: &impl Fn(&Path) -> bool,
    ) -> Vec<FileGrant> {
        plan_file_grants_inner(
            worker_binary,
            descriptor_path,
            args,
            policy,
            &shared_memory_from_forwarded(&args.forwarded_args),
            &state_file_from_forwarded(&args.forwarded_args),
            is_dir,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_file_grants_inner(
        worker_binary: &Path,
        descriptor_path: &Path,
        args: &LauncherArgs,
        policy: Option<&sotf_host::PluginSandboxPolicy>,
        shared_memory: &Option<PathBuf>,
        state_file: &Option<PathBuf>,
        is_dir: &impl Fn(&Path) -> bool,
    ) -> Vec<FileGrant> {
        let mut grants: Vec<FileGrant> = Vec::new();
        let mut push = |path: PathBuf, directory: bool, writable: bool| {
            if !grants.iter().any(|grant| grant.path == path) {
                grants.push(FileGrant {
                    path,
                    directory,
                    writable,
                });
            }
        };

        // The worker image plus its directory (native dependencies sit beside it).
        push(worker_binary.to_path_buf(), false, false);
        if let Some(dir) = worker_binary.parent() {
            push(dir.to_path_buf(), true, false);
        }
        // The third-party plugin bundle itself.
        push(descriptor_path.to_path_buf(), is_dir(descriptor_path), false);

        if let Some(policy) = policy {
            use sotf_host::PluginSandboxFileGrant;
            for grant in &policy.file_access {
                match grant {
                    PluginSandboxFileGrant::PluginBundleReadExecute => {}
                    PluginSandboxFileGrant::PresetDirectoryReadWrite { path }
                    | PluginSandboxFileGrant::ReadWritePath { path } => {
                        push(path.clone(), is_dir(path), true);
                    }
                    PluginSandboxFileGrant::ReadOnlyPath { path } => {
                        push(path.clone(), is_dir(path), false);
                    }
                }
            }
        } else {
            // Legacy flag path: the worker still receives these flags verbatim.
            let mut iter = args.forwarded_args.iter();
            while let Some(arg) = iter.next() {
                if arg == "--sandbox-read-path"
                    && let Some(path) = iter.next()
                {
                    let path = PathBuf::from(path);
                    push(path.clone(), is_dir(&path), false);
                }
                if arg == "--sandbox-write-path"
                    && let Some(path) = iter.next()
                {
                    let path = PathBuf::from(path);
                    push(path.clone(), is_dir(&path), true);
                }
            }
        }

        // The IPC segment: the container needs the session directory and file.
        if let Some(shm) = shared_memory {
            if let Some(dir) = shm.parent() {
                push(dir.to_path_buf(), true, true);
            }
            push(shm.clone(), false, true);
        }
        if let Some(state) = state_file {
            push(state.clone(), false, false);
        }
        grants
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::common::{
        FileGrant, LauncherArgs, ancestor_dirs, plan_file_grants, profile_name_for_plugin,
    };
    use rappct::acl::{AccessMask, ResourcePath, grant_to_package};
    use rappct::launch::{JobLimits, LaunchOptions, StdioConfig, launch_in_container_with_io};
    use rappct::{AppContainerProfile, KnownCapability, SecurityCapabilitiesBuilder};
    use sotf_host::{PluginDescriptor, PluginSandboxNetworkGrant, PluginSandboxPolicy};
    use std::ffi::OsString;

    // winnt.h values; kept local so this binary needs no extra Windows imports.
    const FILE_GENERIC_EXECUTE: u32 = 0x1200A0;
    const FILE_TRAVERSE: u32 = 0x0020;

    pub fn main() {
        if let Err(err) = run(std::env::args().skip(1)) {
            eprintln!("sotf-windows-sandbox-launcher: {err}");
            std::process::exit(1);
        }
    }

    fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
        let args = LauncherArgs::parse(args)?;
        if !args.worker_binary.is_absolute() {
            return Err(format!(
                "external plugin worker path must be absolute: '{}'",
                args.worker_binary.display()
            ));
        }
        let descriptor = load_descriptor(&args)?;
        let policy = load_policy(&args)?;
        let profile_name = profile_name_for_plugin(&descriptor.id);
        let profile = AppContainerProfile::ensure(
            &profile_name,
            "SOTF external plugin worker",
            Some("SOTF isolated external audio plugin worker"),
        )
        .map_err(|err| format!("failed to ensure AppContainer profile '{profile_name}': {err}"))?;

        let mut builder = SecurityCapabilitiesBuilder::new(&profile.sid);
        builder = match rappct::supports_lpac() {
            Ok(()) => builder.lpac(true).with_lpac_defaults(),
            Err(_) => builder.lpac(false),
        };
        if policy
            .as_ref()
            .is_some_and(|policy| policy.network == PluginSandboxNetworkGrant::AnyOutbound)
            || (policy.is_none() && args.allow_network)
        {
            builder = builder.with_known(&[
                KnownCapability::InternetClient,
                KnownCapability::PrivateNetworkClientServer,
            ]);
        }
        let caps = builder
            .build()
            .map_err(|err| format!("failed to build AppContainer capabilities: {err}"))?;

        let grants = plan_file_grants(
            &args.worker_binary,
            &descriptor.path,
            &args,
            policy.as_ref(),
            &|path| path.is_dir(),
        );
        apply_grants(&profile, &grants)?;

        let mut cmdline = String::new();
        super::common::append_quoted(&mut cmdline, &args.worker_binary.to_string_lossy());
        for arg in args.worker_args.iter().chain(args.forwarded_args.iter()) {
            cmdline.push(' ');
            super::common::append_quoted(&mut cmdline, arg);
        }

        let mut env: Vec<(OsString, OsString)> = vec![(
            OsString::from("SOTF_PLUGIN_WORKER"),
            OsString::from("1"),
        )];
        env.extend(
            args.worker_env
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        );
        let opts = LaunchOptions {
            exe: args.worker_binary.clone(),
            cmdline: Some(cmdline),
            cwd: None,
            env: Some(env),
            stdio: StdioConfig::Null,
            suspended: false,
            join_job: Some(JobLimits {
                kill_on_job_close: true,
                ..Default::default()
            }),
            startup_timeout: None,
        };
        let launched = launch_in_container_with_io(&caps, &opts)
            .map_err(|err| format!("failed to launch worker in AppContainer: {err}"))?;
        let code = launched
            .wait(None)
            .map_err(|err| format!("failed waiting for sandboxed worker: {err}"))?;
        std::process::exit(code as i32);
    }

    fn load_descriptor(args: &LauncherArgs) -> Result<PluginDescriptor, String> {
        if let Some(json) = &args.descriptor_json {
            return serde_json::from_str(json)
                .map_err(|err| format!("failed to parse descriptor JSON: {err}"));
        }
        if let Some(path) = &args.descriptor_file {
            let json = std::fs::read_to_string(path).map_err(|err| {
                format!("failed to read descriptor file '{}': {err}", path.display())
            })?;
            return serde_json::from_str(&json)
                .map_err(|err| format!("failed to parse descriptor file: {err}"));
        }
        Err("missing --descriptor-json or --descriptor-file".to_string())
    }

    fn load_policy(args: &LauncherArgs) -> Result<Option<PluginSandboxPolicy>, String> {
        if let Some(json) = &args.sandbox_policy_json {
            let policy: PluginSandboxPolicy = serde_json::from_str(json)
                .map_err(|err| format!("failed to parse sandbox policy JSON: {err}"))?;
            return Ok(Some(policy));
        }
        Ok(None)
    }

    fn apply_grants(profile: &AppContainerProfile, grants: &[FileGrant]) -> Result<(), String> {
        for grant in grants {
            let target = if grant.directory {
                ResourcePath::Directory(grant.path.clone())
            } else {
                ResourcePath::File(grant.path.clone())
            };
            grant_to_package(target, &profile.sid, access_for(grant))
                .map_err(|err| format!("failed to grant '{}': {err}", grant.path.display()))?;
            for ancestor in ancestor_dirs(&grant.path) {
                grant_to_package(
                    ResourcePath::Directory(ancestor.clone()),
                    &profile.sid,
                    AccessMask(FILE_TRAVERSE),
                )
                .map_err(|err| {
                    format!("failed to grant traverse on '{}': {err}", ancestor.display())
                })?;
            }
        }
        Ok(())
    }

    fn access_for(grant: &FileGrant) -> AccessMask {
        let read = AccessMask::FILE_GENERIC_READ.0 | FILE_GENERIC_EXECUTE;
        if grant.writable {
            AccessMask(read | AccessMask::FILE_GENERIC_WRITE.0)
        } else {
            AccessMask(read)
        }
    }
}

#[cfg(target_os = "windows")]
fn main() {
    windows::main();
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("sotf-windows-sandbox-launcher is only supported on Windows");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::common::*;
    use std::path::{Path, PathBuf};

    fn args(flags: &[&str]) -> LauncherArgs {
        LauncherArgs::parse(flags.iter().map(|flag| flag.to_string())).unwrap()
    }

    #[test]
    fn parses_worker_metadata_and_forwards_worker_args() {
        let parsed = args(&[
            "--sandbox-worker-binary",
            "C:\\sotf\\worker.exe",
            "--sandbox-worker-arg",
            "--idle-sleep-micros",
            "--sandbox-worker-arg",
            "50",
            "--sandbox-worker-env",
            "SOTF_TEST=value=with=equals",
            "--descriptor-json",
            "{}",
            "--shared-memory",
            "C:\\Temp\\sotf.shm",
        ]);

        assert_eq!(parsed.worker_binary, PathBuf::from("C:\\sotf\\worker.exe"));
        assert_eq!(parsed.worker_args, vec!["--idle-sleep-micros", "50"]);
        assert_eq!(
            parsed.worker_env,
            vec![("SOTF_TEST".to_string(), "value=with=equals".to_string())]
        );
        assert_eq!(
            parsed.forwarded_args,
            vec![
                "--descriptor-json",
                "{}",
                "--shared-memory",
                "C:\\Temp\\sotf.shm",
            ]
        );
        assert_eq!(parsed.descriptor_json.as_deref(), Some("{}"));
        assert_eq!(
            shared_memory_from_forwarded(&parsed.forwarded_args),
            Some(PathBuf::from("C:\\Temp\\sotf.shm"))
        );
    }

    #[test]
    fn rejects_missing_worker_binary() {
        let err = LauncherArgs::parse(["--descriptor-json".to_string(), "{}".to_string()])
            .unwrap_err();

        assert!(err.contains("missing --sandbox-worker-binary"));
    }

    #[test]
    fn rejects_invalid_worker_env() {
        let err = LauncherArgs::parse([
            "--sandbox-worker-binary".to_string(),
            "C:\\sotf\\worker.exe".to_string(),
            "--sandbox-worker-env".to_string(),
            "SOTF_TEST".to_string(),
        ])
        .unwrap_err();

        assert!(err.contains("KEY=VALUE"));
    }

    #[test]
    fn skims_legacy_network_flag_without_consuming_it() {
        let parsed = args(&[
            "--sandbox-worker-binary",
            "C:\\sotf\\worker.exe",
            "--sandbox-allow-network",
        ]);

        assert!(parsed.allow_network);
        assert!(
            parsed
                .forwarded_args
                .contains(&"--sandbox-allow-network".to_string())
        );
    }

    #[test]
    fn profile_name_sanitizes_plugin_id() {
        assert_eq!(
            profile_name_for_plugin("com.vendor.My Plugin:v2"),
            "sotf.plugin.com.vendor.My_Plugin_v2"
        );
        assert_eq!(
            profile_name_for_plugin("!!!"),
            "sotf.plugin.external-plugin"
        );
    }

    #[test]
    fn command_line_quoting_round_trips_spaces_and_quotes() {
        let mut cmdline = String::new();
        append_quoted(&mut cmdline, "simple");
        cmdline.push(' ');
        append_quoted(&mut cmdline, "C:\\Program Files\\plug.dll");
        cmdline.push(' ');
        append_quoted(&mut cmdline, "say \"hi\"");

        assert_eq!(
            cmdline,
            "simple \"C:\\Program Files\\plug.dll\" \"say \\\"hi\\\"\""
        );
    }

    #[test]
    fn ancestor_dirs_stops_before_root() {
        let ancestors = ancestor_dirs(Path::new("/tmp/sotf/session-1/audio.shm"));

        assert_eq!(
            ancestors,
            vec![
                PathBuf::from("/tmp/sotf/session-1"),
                PathBuf::from("/tmp/sotf"),
                PathBuf::from("/tmp"),
            ]
        );
        assert!(ancestor_dirs(Path::new("/")).is_empty());
    }

    #[test]
    fn grant_plan_covers_worker_bundle_ipc_and_state() {
        let parsed = args(&[
            "--sandbox-worker-binary",
            "/opt/sotf/worker",
            "--descriptor-json",
            "{}",
            "--shared-memory",
            "/tmp/sotf/session-1/audio.shm",
            "--external-state-file",
            "/tmp/sotf/state.json",
        ]);
        let is_dir = |path: &Path| path.extension().is_none();
        let grants = plan_file_grants(
            Path::new("/opt/sotf/worker"),
            Path::new("/opt/plugins/fake.vst3"),
            &parsed,
            None,
            &is_dir,
        );

        for expected in [
            ("/opt/sotf/worker", false, false),
            ("/opt/sotf", true, false),
            ("/opt/plugins/fake.vst3", false, false),
            ("/tmp/sotf/session-1", true, true),
            ("/tmp/sotf/session-1/audio.shm", false, true),
            ("/tmp/sotf/state.json", false, false),
        ] {
            assert!(
                grants.contains(&FileGrant {
                    path: PathBuf::from(expected.0),
                    directory: expected.1,
                    writable: expected.2,
                }),
                "missing grant {expected:?} in {grants:?}"
            );
        }
    }
}
