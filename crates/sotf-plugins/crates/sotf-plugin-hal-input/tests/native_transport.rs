//! Explicitly isolated production broker/reader integration, never live HAL state.
#![cfg(all(target_os = "macos", feature = "hal"))]

use driver_hal::{SharedAudioBuffer, encryption::AudioCipher};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_hal_input::HalInputPlugin;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

fn key(path: &Path, value: u8) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&[value; 32]).unwrap();
}
fn data(channels: usize, frames: usize, epoch: usize) -> Vec<f32> {
    (0..frames * channels).map(|i| (epoch + i) as f32).collect()
}
fn read(p: &mut HalInputPlugin, rate: u32, frames: usize) -> Vec<f32> {
    let mut out = vec![99.0; frames * p.output_channels()];
    sotf_host::assert_no_allocs("native HAL input process", || {
        assert_eq!(
            p.process(&[], &mut out, &ProcessContext::new(rate, frames)),
            Ok(frames)
        );
    });
    out
}
fn expect_prefix(output: &[f32], expected: &[f32]) {
    assert_eq!(&output[..expected.len()], expected);
    assert!(output[expected.len()..].iter().all(|&v| v == 0.0));
}

fn writer_worker(runtime: &Path, epoch: usize) {
    let path = runtime.join("audio.shm");
    assert_eq!(driver_hal::get_shared_memory_path(), path);
    assert_eq!(
        driver_hal::encryption::get_session_key_path(),
        runtime.join("session.key")
    );
    let mut writer =
        SharedAudioBuffer::create_or_open_with_max_geometry(&path, 48_000, 64, 2, 128, 16).unwrap();
    writer.header().driver_ready.store(1, Ordering::Release);
    assert_eq!(writer.write_audio(&data(2, 8, epoch)), 8);
    std::fs::write(runtime.join(format!("writer-ready-{epoch}")), b"ready").unwrap();
    // The parent keeps stdin open. EOF also retires the worker if its parent
    // exits unexpectedly, so the fixture cannot leave an orphan broker.
    let mut command = String::new();
    let _ = std::io::stdin().read_line(&mut command);
}

fn spawn_writer(runtime: &Path, epoch: usize) -> std::process::Child {
    use std::process::Stdio;
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "isolated_brokered_transport",
            "--nocapture",
        ])
        .env("SOTF_HAL_INPUT_AUDIT_WRITER", epoch.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !runtime.join(format!("writer-ready-{epoch}")).exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "fixture writer exited before publication"
        );
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fixture writer publication timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child
}

fn abrupt_writer_death_and_replacement(runtime: &Path) {
    let mut writer = spawn_writer(runtime, 8000);
    let mut p = HalInputPlugin::new(2).unwrap();
    p.initialize(48_000.0).unwrap();
    expect_prefix(&read(&mut p, 48_000, 3), &data(2, 8, 8000)[..6]);
    writer.kill().unwrap();
    writer.wait().unwrap();
    // The immutable mapped object remains owned by the reader after its broker
    // dies. Its already-published complete frames are still valid to consume.
    expect_prefix(&read(&mut p, 48_000, 8), &data(2, 8, 8000)[6..]);
    let before = p.diagnostics();
    assert!(read(&mut p, 48_000, 8).iter().all(|&v| v == 0.0));
    assert_eq!(p.diagnostics().missing_frames, before.missing_frames + 8);
    let automatic_recovery = p.diagnostics().needs_control_recovery;
    assert!(p.service_transport().is_err());
    let serviced = p.diagnostics();
    assert!(!serviced.connected && serviced.needs_control_recovery);
    // The pinned old header still says ready; callback observation cannot
    // override the failed broker service or turn this into an ordinary underrun.
    let missing = serviced.missing_frames;
    let underruns = serviced.underrun_count;
    for _ in 0..3 {
        assert_eq!(read(&mut p, 48_000, 8), vec![0.0; 16]);
        assert!(p.diagnostics().needs_control_recovery);
        assert!(!p.diagnostics().connected);
    }
    assert_eq!(p.diagnostics().missing_frames, missing + 24);
    assert_eq!(p.diagnostics().underrun_count, underruns);
    let mut replacement = spawn_writer(runtime, 9000);
    p.service_transport().unwrap();
    expect_prefix(&read(&mut p, 48_000, 8), &data(2, 8, 9000));
    replacement.kill().unwrap();
    replacement.wait().unwrap();
    println!(
        "native_hal_input_cross_process abrupt_writer_death=true immutable_region_retained=true queued_frames_exact=true replacement_first_frame_exact=true automatic_recovery={automatic_recovery} failed_service_latched=true successful_service_first_frame_exact=true"
    );
}

fn native_cases(runtime: &Path) {
    let path = runtime.join("audio.shm");
    let key_path = runtime.join("session.key");
    assert_eq!(driver_hal::get_shared_memory_path(), path);
    assert_eq!(driver_hal::encryption::get_session_key_path(), key_path);
    assert_eq!(
        std::env::var_os("SOTF_SYSTEMWIDE_RUNTIME_DIR").map(PathBuf::from),
        Some(runtime.to_path_buf())
    );
    let mut cases = 0;
    for rate in [48_000_u32, 96_000] {
        for channels in 1..=16 {
            let mut writer = SharedAudioBuffer::create_or_open_with_max_geometry(
                &path,
                rate,
                64,
                channels as u32,
                128,
                16,
            )
            .unwrap();
            writer.header().driver_ready.store(1, Ordering::Release);
            let mut p = HalInputPlugin::new(channels).unwrap();
            p.initialize(f64::from(rate)).unwrap();
            assert!(read(&mut p, rate, 8).iter().all(|&v| v == 0.0));
            assert_eq!(p.diagnostics().missing_frames, 8);
            assert_eq!(p.underrun_count(), 0);
            let first = data(channels, 7, 1000);
            assert_eq!(writer.write_audio(&first), 7);
            expect_prefix(&read(&mut p, rate, 3), &first[..3 * channels]);
            expect_prefix(&read(&mut p, rate, 8), &first[3 * channels..]);
            assert_eq!(p.underrun_count(), 1);
            writer.header().driver_ready.store(0, Ordering::Release);
            let before = p.diagnostics();
            assert!(read(&mut p, rate, 8).iter().all(|&v| v == 0.0));
            let disconnected = p.diagnostics();
            assert_eq!(disconnected.missing_frames, before.missing_frames + 8);
            assert!(disconnected.needs_control_recovery);
            assert_eq!(disconnected.underrun_count, before.underrun_count);
            writer.header().driver_ready.store(1, Ordering::Release);
            let fresh = data(channels, 5, 2000);
            assert_eq!(writer.write_audio(&fresh), 5);
            expect_prefix(&read(&mut p, rate, 8), &fresh);
            key(&key_path, 0x11);
            let cipher_a = AudioCipher::new(&[0x11; 32]);
            writer.set_key_fingerprint(*cipher_a.fingerprint());
            writer.set_encrypted(true);
            writer.flush_audio();
            assert!(read(&mut p, rate, 8).iter().all(|&v| v == 0.0));
            assert!(p.diagnostics().needs_control_recovery);
            p.refresh_transport().unwrap();
            let encrypted = data(channels, 7, 3000);
            assert_eq!(writer.write_audio_encrypted(&encrypted, &cipher_a), 7);
            expect_prefix(&read(&mut p, rate, 3), &encrypted[..3 * channels]);
            // Leave old decrypted record suffix staged; rotate without publishing it.
            key(&key_path, 0x22);
            let cipher_b = AudioCipher::new(&[0x22; 32]);
            writer.set_key_fingerprint(*cipher_b.fingerprint());
            writer.set_encrypted(true);
            writer.flush_audio();
            let rotated = data(channels, 5, 4000);
            assert_eq!(writer.write_audio_encrypted(&rotated, &cipher_b), 5);
            assert!(read(&mut p, rate, 8).iter().all(|&v| v == 0.0));
            p.refresh_transport().unwrap();
            expect_prefix(&read(&mut p, rate, 8), &rotated);
            // Failed key refresh must retain the valid active B reader.
            key(&key_path, 0x11);
            let before = p.diagnostics();
            assert!(p.refresh_transport().is_err());
            assert_eq!(p.diagnostics(), before);
            key(&key_path, 0x22);
            let retained = data(channels, 5, 5000);
            assert_eq!(writer.write_audio_encrypted(&retained, &cipher_b), 5);
            expect_prefix(&read(&mut p, rate, 8), &retained);
            // Healthy service probes must keep the active decrypted suffix;
            // reopening it unconditionally would lose these four source frames.
            let healthy = data(channels, 7, 5500);
            assert_eq!(writer.write_audio_encrypted(&healthy, &cipher_b), 7);
            expect_prefix(&read(&mut p, rate, 3), &healthy[..3 * channels]);
            let before_service = p.diagnostics();
            p.service_transport().unwrap();
            assert_eq!(p.diagnostics(), before_service);
            expect_prefix(&read(&mut p, rate, 8), &healthy[3 * channels..]);
            let other_rate = if rate == 48_000 { 96_000 } else { 48_000 };
            writer.set_sample_rate(other_rate);
            let mut rejected = vec![99.0; 8 * channels];
            assert!(
                p.process(&[], &mut rejected, &ProcessContext::new(rate, 8))
                    .is_err()
            );
            assert!(rejected.iter().all(|&v| v == 0.0));
            assert!(p.refresh_transport().is_err());
            writer.set_sample_rate(rate);
            let restored = data(channels, 5, 6000);
            assert_eq!(writer.write_audio_encrypted(&restored, &cipher_b), 5);
            expect_prefix(&read(&mut p, rate, 8), &restored);
            writer.header().driver_ready.store(0, Ordering::Release);
            assert!(read(&mut p, rate, 8).iter().all(|&v| v == 0.0));
            let mut replacement = SharedAudioBuffer::create_or_open_with_max_geometry(
                &path,
                rate,
                64,
                channels as u32,
                128,
                16,
            )
            .unwrap();
            replacement
                .header()
                .driver_ready
                .store(1, Ordering::Release);
            let restart = data(channels, 5, 7000);
            assert_eq!(replacement.write_audio(&restart), 5);
            p.refresh_transport().unwrap();
            expect_prefix(&read(&mut p, rate, 8), &restart);
            drop(writer);
            drop(replacement);
            let before = p.diagnostics();
            let silence = read(&mut p, rate, 8);
            assert!(silence.iter().all(|&v| v == 0.0));
            let after = p.diagnostics();
            assert_eq!(after.missing_frames, before.missing_frames + 8);
            println!(
                "native_hal_input rate={rate} channels={channels} plaintext=true encrypted=true old_key_staging_discarded=true restart_first_frame_exact=true refresh_failure_retained=true format_rejected=true healthy_service_staged_suffix_exact=true after_writer_drop_recovery={}",
                after.needs_control_recovery
            );
            drop(p);
            cases += 1;
        }
    }
    abrupt_writer_death_and_replacement(runtime);
    println!(
        "native_hal_input_summary cases={cases} live_transport_opened=false dependency_snapshot=worktree"
    );
}

#[test]
#[ignore = "requires explicit isolated SOTF_HAL_AUDIT_FIXTURE_ROOT; run this target alone"]
fn isolated_brokered_transport() {
    let root = PathBuf::from(
        std::env::var_os("SOTF_HAL_AUDIT_FIXTURE_ROOT").expect("explicit fixture root required"),
    );
    assert!(root.is_absolute() && root.is_dir());
    let runtime = root.join("transport");
    if std::env::var_os("SOTF_HAL_INPUT_AUDIT_CHILD").is_some() {
        assert!(runtime.is_dir());
        if let Some(epoch) = std::env::var_os("SOTF_HAL_INPUT_AUDIT_WRITER") {
            writer_worker(&runtime, epoch.to_str().unwrap().parse().unwrap());
        } else {
            native_cases(&runtime);
        }
        return;
    }
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime)
        .unwrap();
    let stdout = std::fs::File::create(root.join("child.stdout")).unwrap();
    let stderr = std::fs::File::create(root.join("child.stderr")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "isolated_brokered_transport",
            "--nocapture",
        ])
        .env("SOTF_HAL_INPUT_AUDIT_CHILD", "1")
        .env("SOTF_SYSTEMWIDE_RUNTIME_DIR", &runtime)
        .env("SOTF_HAL_SHARED_MEMORY_PATH", runtime.join("audio.shm"))
        .env("SOTF_HAL_SESSION_KEY_PATH", runtime.join("session.key"))
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("isolated native child exceeded 45s");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    println!(
        "{}",
        std::fs::read_to_string(root.join("child.stdout")).unwrap()
    );
    eprintln!(
        "{}",
        std::fs::read_to_string(root.join("child.stderr")).unwrap()
    );
    assert!(status.success(), "isolated native child failed: {status}");
    std::fs::remove_dir_all(&runtime).unwrap();
}
