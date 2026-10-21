//! Isolated native output ownership and control remapping, never live HAL output.
#![cfg(all(target_os = "macos", feature = "hal"))]
use driver_hal::{SharedAudioBuffer, encryption::AudioCipher};
use sotf_host::plugin::{SinkTransportFormat, SinkTransportRecoveryStatus, TerminalSink};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_hal_output::HalOutputPlugin;
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
fn data(ch: usize, frames: usize, epoch: usize) -> Vec<f32> {
    (0..ch * frames).map(|i| (epoch + i) as f32).collect()
}
fn process(p: &mut HalOutputPlugin, input: &[f32], rate: u32, ch: usize) {
    sotf_host::assert_no_allocs("native HAL output process", || {
        assert_eq!(
            p.process(input, &mut [], &ProcessContext::new(rate, input.len() / ch)),
            Ok(input.len() / ch)
        );
    });
}
fn take(peer: &SharedAudioBuffer, ch: usize, frames: usize) -> Vec<f32> {
    let mut out = vec![99.0; ch * frames];
    let n = peer.read_audio(&mut out);
    out.truncate(n * ch);
    out
}
fn prime(peer: &SharedAudioBuffer, ch: usize) {
    let frames = peer.buffer_frames() as usize;
    let out = take(peer, ch, frames);
    assert_eq!(out.len(), frames * ch);
    assert!(out.iter().all(|v| *v == 0.0));
}
fn encrypted_prime(peer: &SharedAudioBuffer, ch: usize, cipher: &AudioCipher) {
    let frames = peer.buffer_frames() as usize;
    let mut out = vec![99.0; frames * ch];
    assert_eq!(peer.read_audio_encrypted(&mut out, cipher), frames);
    assert!(out.iter().all(|v| *v == 0.0));
}
fn encrypted_take(
    peer: &SharedAudioBuffer,
    ch: usize,
    frames: usize,
    cipher: &AudioCipher,
) -> Vec<f32> {
    let mut out = vec![99.0; frames * ch];
    assert_eq!(peer.read_audio_encrypted(&mut out, cipher), frames);
    out
}
fn conservation(p: &HalOutputPlugin) {
    let t = p.telemetry();
    assert_eq!(
        t.requested_frames,
        t.written_frames + t.queued_frames as u64 + t.dropped_frames
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
        for ch in 1..=16 {
            let mut peer = SharedAudioBuffer::create_or_open_with_max_geometry(
                &path, rate, 64, ch as u32, 128, 16,
            )
            .unwrap();
            peer.header().driver_ready.store(1, Ordering::Release);
            let mut p = HalOutputPlugin::new(ch).unwrap();
            p.initialize(f64::from(rate)).unwrap();
            prime(&peer, ch);
            assert_eq!(p.latency_samples(), 128);
            // Saturate the physical ring before exercising the logical queue's policy.
            let fill = vec![-1.0; 64 * ch];
            let mut filled = 0;
            loop {
                let n = peer.write_audio(&fill);
                filled += n;
                if n == 0 {
                    break;
                }
                assert!(filled < 100_000);
            }
            let capacity = p.telemetry().queue_capacity_frames;
            let input = data(ch, capacity + 17, 1000);
            process(&mut p, &input, rate, ch);
            assert_eq!(p.telemetry().queued_frames, capacity);
            assert_eq!(p.telemetry().dropped_frames, 17);
            conservation(&p);
            let mut removed = 0;
            loop {
                let out = take(&peer, ch, 17);
                if out.is_empty() {
                    break;
                }
                assert!(out.iter().all(|v| *v == -1.0));
                removed += out.len() / ch;
            }
            assert_eq!(removed, filled);
            let mut actual = Vec::new();
            for _ in 0..capacity + 1 {
                process(&mut p, &[], rate, ch);
                loop {
                    let out = take(&peer, ch, 17);
                    if out.is_empty() {
                        break;
                    }
                    actual.extend(out);
                }
                if p.telemetry().queued_frames == 0 {
                    break;
                }
            }
            assert_eq!(actual, &input[..capacity * ch]);
            conservation(&p);
            // A daemon can replace the mapping while the retired header still says ready.
            peer.header().driver_ready.store(0, Ordering::Release);
            let pending = data(ch, 5, 20_000);
            process(&mut p, &pending, rate, ch);
            peer.header().driver_ready.store(1, Ordering::Release);
            let replacement = SharedAudioBuffer::create_or_open_with_max_geometry(
                &path, rate, 64, ch as u32, 128, 16,
            )
            .unwrap();
            replacement
                .header()
                .driver_ready
                .store(1, Ordering::Release);
            p.service_transport().unwrap();
            assert!(
                replacement.engine_ready(),
                "explicit service must publish readiness on the replacement mapping"
            );
            assert!(
                !peer.engine_ready(),
                "retired mapping must lose engine readiness"
            );
            prime(&replacement, ch);
            process(&mut p, &[], rate, ch);
            assert_eq!(take(&replacement, ch, 5), pending);
            conservation(&p);
            // Repeat through the TerminalSink control contract.
            replacement
                .header()
                .driver_ready
                .store(0, Ordering::Release);
            let pending = data(ch, 6, 30_000);
            process(&mut p, &pending, rate, ch);
            replacement
                .header()
                .driver_ready
                .store(1, Ordering::Release);
            let mut current = SharedAudioBuffer::create_or_open_with_max_geometry(
                &path, rate, 64, ch as u32, 128, 16,
            )
            .unwrap();
            current.header().driver_ready.store(1, Ordering::Release);
            assert_eq!(
                p.recover_transport(SinkTransportFormat {
                    sample_rate: rate,
                    channels: ch,
                    buffer_frames: 64
                })
                .unwrap(),
                SinkTransportRecoveryStatus::Ready
            );
            assert!(!replacement.engine_ready());
            assert!(current.engine_ready());
            prime(&current, ch);
            process(&mut p, &[], rate, ch);
            assert_eq!(take(&current, ch, 6), pending);
            conservation(&p);
            // Reprepare must also attach the current region, even when its
            // geometry matches the cached plan and the old bit stays ready.
            current.set_buffer_frames(128);
            current.header().config_changed.store(1, Ordering::Release);
            let plan = p.reprepare_plan().unwrap();
            let previous = current;
            current = SharedAudioBuffer::create_or_open_with_max_geometry(
                &path, rate, 128, ch as u32, 128, 16,
            )
            .unwrap();
            current.header().driver_ready.store(1, Ordering::Release);
            current.header().config_changed.store(1, Ordering::Release);
            assert_eq!(
                p.reprepare_transport(plan).unwrap(),
                SinkTransportRecoveryStatus::Ready
            );
            assert!(!previous.engine_ready());
            assert!(current.engine_ready());
            assert_eq!(p.latency_samples(), 256);
            prime(&current, ch);
            key(&key_path, 0x11);
            let a = AudioCipher::new(&[0x11; 32]);
            current.set_key_fingerprint(*a.fingerprint());
            current.set_encrypted(true);
            current.flush_audio();
            let pending = data(ch, 5, 40_000);
            process(&mut p, &pending, rate, ch);
            p.service_transport().unwrap();
            encrypted_prime(&current, ch, &a);
            process(&mut p, &[], rate, ch);
            assert_eq!(encrypted_take(&current, ch, 5, &a), pending);
            let b = AudioCipher::new(&[0x22; 32]);
            current.set_key_fingerprint(*b.fingerprint());
            current.flush_audio();
            let pending = data(ch, 5, 50_000);
            process(&mut p, &pending, rate, ch);
            assert!(p.service_transport().is_err());
            assert_eq!(p.telemetry().queued_frames, 5);
            key(&key_path, 0x22);
            p.service_transport().unwrap();
            encrypted_prime(&current, ch, &b);
            process(&mut p, &[], rate, ch);
            assert_eq!(encrypted_take(&current, ch, 5, &b), pending);
            conservation(&p);
            let other = if rate == 48_000 { 96_000 } else { 48_000 };
            current.set_sample_rate(other);
            // Geometry mutation and notification are separate owner operations.
            current.header().config_changed.store(1, Ordering::Release);
            let first = data(ch, 5, 60_000);
            process(&mut p, &first, rate, ch);
            current.clear_config_changed();
            let second = data(ch, 5, 70_000);
            process(&mut p, &second, rate, ch);
            assert!(p.service_transport().is_err());
            assert!(!current.engine_ready());
            current.set_sample_rate(rate);
            p.service_transport().unwrap();
            encrypted_prime(&current, ch, &b);
            process(&mut p, &[], rate, ch);
            let mut expected = first;
            expected.extend(second);
            assert_eq!(encrypted_take(&current, ch, 10, &b), expected);
            conservation(&p);
            println!(
                "native_hal_output rate={rate} channels={ch} saturation_fifo_exact=true drop_newest=17 service_remap=true terminal_remap=true reprepare_remap=true retired_readiness_cleared=true key_recovery=true config_latch=true callback_allocations=0"
            );
            drop(p);
            assert!(!current.engine_ready());
            cases += 1;
        }
    }
    println!(
        "native_hal_output_summary cases={cases} live_transport_opened=false dependency_snapshot=worktree"
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
    if std::env::var_os("SOTF_HAL_OUTPUT_AUDIT_CHILD").is_some() {
        assert!(runtime.is_dir());
        native_cases(&runtime);
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
        .env("SOTF_HAL_OUTPUT_AUDIT_CHILD", "1")
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
