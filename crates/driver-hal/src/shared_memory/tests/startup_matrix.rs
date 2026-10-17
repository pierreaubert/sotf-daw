//! Startup combination matrix for the daemon <-> coreaudiod exchange file.
//!
//! Actors: the daemon side (`create_or_open`, may create/repair the file)
//! and the HAL side running in coreaudiod (`open`, open-only: it must never
//! create, resize, or re-permission the file). Every combination of
//! pre-existing file state, crypto-key state, and start order must end in
//! a sane, private, working transport — or fail closed without creating
//! anything — and never panic. These are the states a reboot, a crash, an
//! OS upgrade, a key rotation, or a same-user mistake can leave behind.

use super::super::consts::{SHARED_MEMORY_MAGIC, SHARED_MEMORY_VERSION};
use super::super::shared_audio_buffer::SharedAudioBuffer;
use crate::encryption::AudioCipher;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tempfile::tempdir;

const GEOMETRY_48K: (u32, u32, u32) = (48_000, 512, 2);
const GEOMETRY_96K: (u32, u32, u32) = (96_000, 1024, 2);
const KEY_A: [u8; 32] = [0xA1; 32];
const KEY_B: [u8; 32] = [0xB2; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileState {
    Absent,
    Empty,
    Truncated,
    Garbage,
    WrongVersion,
    Valid48k,
    Valid96k,
    StaleFlags,
    ZeroRate,
    ZeroChannels,
    HugeChannels,
    WorldWritable,
    #[cfg(unix)]
    Symlink,
}

impl FileState {
    fn all() -> Vec<Self> {
        vec![
            Self::Absent,
            Self::Empty,
            Self::Truncated,
            Self::Garbage,
            Self::WrongVersion,
            Self::Valid48k,
            Self::Valid96k,
            Self::StaleFlags,
            Self::ZeroRate,
            Self::ZeroChannels,
            Self::HugeChannels,
            Self::WorldWritable,
            #[cfg(unix)]
            Self::Symlink,
        ]
    }

    /// Whether the pre-existing header counts as a valid live transport the
    /// daemon must adopt (geometry and crypto state preserved).
    fn is_adoptable(self) -> bool {
        matches!(self, Self::Valid48k | Self::Valid96k | Self::StaleFlags)
    }

    fn expected_geometry(self) -> (u32, u32, u32) {
        match self {
            Self::Valid48k => GEOMETRY_48K,
            Self::Valid96k | Self::StaleFlags => GEOMETRY_96K,
            _ => GEOMETRY_48K,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyState {
    Plaintext,
    Matching,
    Mismatched,
    FlaggedNoFingerprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartOrder {
    DaemonFirst,
    HalFirst,
    DaemonRestart,
}

fn set_mode_0600(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("chmod 0600");
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn write_raw(path: &Path, bytes: &[u8]) {
    let mut file = fs::File::create(path).expect("create raw fixture");
    file.write_all(bytes).expect("write raw fixture");
    drop(file);
    set_mode_0600(path);
}

fn create_valid(path: &Path, geometry: (u32, u32, u32)) {
    let buffer = SharedAudioBuffer::create_or_open(path, geometry.0, geometry.1, geometry.2)
        .expect("create valid fixture");
    drop(buffer);
}

/// Materialize `state` at `path`; returns true when the file is adoptable.
fn prepare_file_state(dir: &Path, path: &Path, state: FileState) {
    match state {
        FileState::Absent => {}
        FileState::Empty => write_raw(path, &[]),
        FileState::Truncated => write_raw(path, &[0x53; 16]),
        FileState::Garbage => write_raw(path, &[0xAB; 8192]),
        FileState::WrongVersion => {
            create_valid(path, GEOMETRY_48K);
            let buffer = SharedAudioBuffer::open(path).expect("open fixture");
            buffer
                .header()
                .version
                .store(SHARED_MEMORY_VERSION + 1, Ordering::Release);
        }
        FileState::Valid48k => create_valid(path, GEOMETRY_48K),
        FileState::Valid96k => create_valid(path, GEOMETRY_96K),
        FileState::StaleFlags => {
            create_valid(path, GEOMETRY_96K);
            let buffer = SharedAudioBuffer::open(path).expect("open fixture");
            let header = buffer.header();
            header.driver_ready.store(1, Ordering::Release);
            header.active.store(1, Ordering::Release);
            header.engine_ready.store(1, Ordering::Release);
            header.frame_counter.store(123_456, Ordering::Release);
        }
        FileState::ZeroRate => {
            create_valid(path, GEOMETRY_48K);
            let mut buffer = SharedAudioBuffer::open(path).expect("open fixture");
            buffer.set_sample_rate(0);
        }
        FileState::ZeroChannels => {
            create_valid(path, GEOMETRY_48K);
            let buffer = SharedAudioBuffer::open(path).expect("open fixture");
            buffer.header().channel_count.store(0, Ordering::Release);
        }
        FileState::HugeChannels => {
            create_valid(path, GEOMETRY_48K);
            let buffer = SharedAudioBuffer::open(path).expect("open fixture");
            buffer.header().channel_count.store(999, Ordering::Release);
        }
        FileState::WorldWritable => {
            create_valid(path, GEOMETRY_96K);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o666)).expect("chmod 0666");
            }
        }
        #[cfg(unix)]
        FileState::Symlink => {
            let target = dir.join("real-audio.shm");
            create_valid(&target, GEOMETRY_48K);
            std::os::unix::fs::symlink(&target, path).expect("create symlink fixture");
        }
    }
    let _ = dir;
}

fn apply_key_state(path: &Path, key: KeyState) {
    if !path.exists() {
        return;
    }
    let Ok(buffer) = SharedAudioBuffer::open(path) else {
        return;
    };
    let cipher_a = AudioCipher::new(&KEY_A);
    match key {
        KeyState::Plaintext => {}
        KeyState::Matching | KeyState::Mismatched => {
            buffer.set_encrypted(true);
            buffer.set_key_fingerprint(*cipher_a.fingerprint());
        }
        KeyState::FlaggedNoFingerprint => {
            buffer.set_encrypted(true);
            buffer.set_key_fingerprint([0; 8]);
        }
    }
}

/// Mirror of the daemon's HAL startup: adopt a readable live geometry,
/// otherwise fall back to the default, then create-or-open. This is the
/// `initial_shared_memory_geometry` + `create_or_open_default` sequence.
fn daemon_start(path: &Path) -> std::io::Result<SharedAudioBuffer> {
    let geometry = SharedAudioBuffer::open(path)
        .map(|buffer| {
            (
                buffer.sample_rate(),
                buffer.buffer_frames(),
                buffer.channel_count(),
            )
        })
        .ok()
        .filter(|&(rate, frames, channels)| rate > 0 && frames > 0 && channels > 0)
        .unwrap_or(GEOMETRY_48K);
    SharedAudioBuffer::create_or_open(path, geometry.0, geometry.1, geometry.2)
}

fn case_name(state: FileState, key: KeyState, order: StartOrder) -> String {
    format!("{state:?} x {key:?} x {order:?}")
}

fn sample_block(frames: usize, channels: u32) -> Vec<f32> {
    (0..frames * channels as usize)
        .map(|i| (i as f32 * 0.01).sin() * 0.5)
        .collect()
}

fn assert_transport_works(
    daemon: &mut SharedAudioBuffer,
    hal: &mut SharedAudioBuffer,
    key: KeyState,
    preserved: bool,
    case: &str,
) {
    let frames = 64usize;
    let channels = daemon.channel_count();
    let samples = sample_block(frames, channels);
    let mut out = vec![0.0f32; samples.len()];
    let cipher_a = AudioCipher::new(&KEY_A);
    let cipher_b = AudioCipher::new(&KEY_B);

    if daemon.is_encrypted() {
        assert!(preserved, "{case}: encryption survived a header re-init");
        assert_eq!(
            hal.write_audio_encrypted(&samples, &cipher_a),
            frames,
            "{case}: encrypted write"
        );
        let daemon_cipher = match key {
            KeyState::Mismatched => &cipher_b,
            _ => &cipher_a,
        };
        let read = daemon.read_audio_encrypted(&mut out, daemon_cipher);
        if key == KeyState::Mismatched {
            // Wrong key (rotation the HAL has not picked up yet) must drop
            // audio, never deliver garbage or the plaintext.
            assert_eq!(read, 0, "{case}: mismatched key must not yield frames");
            assert_ne!(out, samples, "{case}: mismatched key leaked plaintext");
        } else {
            assert_eq!(read, frames, "{case}: encrypted read");
            assert_eq!(out, samples, "{case}: encrypted roundtrip must be exact");
        }
    } else {
        assert_eq!(hal.write_audio(&samples), frames, "{case}: plaintext write");
        assert_eq!(
            daemon.read_audio(&mut out),
            frames,
            "{case}: plaintext read"
        );
        assert_eq!(out, samples, "{case}: plaintext roundtrip must be exact");
    }
}

#[test]
fn startup_matrix_file_x_key_x_order() {
    let dir = tempdir().expect("create matrix temp dir");
    for state in FileState::all() {
        for key in [
            KeyState::Plaintext,
            KeyState::Matching,
            KeyState::Mismatched,
            KeyState::FlaggedNoFingerprint,
        ] {
            for order in [
                StartOrder::DaemonFirst,
                StartOrder::HalFirst,
                StartOrder::DaemonRestart,
            ] {
                let case = case_name(state, key, order);
                let case_dir: PathBuf = dir.path().join(format!("{state:?}-{key:?}-{order:?}"));
                fs::create_dir_all(&case_dir).expect("case dir");
                let path = case_dir.join("audio.shm");
                prepare_file_state(&case_dir, &path, state);
                if state.is_adoptable() {
                    apply_key_state(&path, key);
                }

                // Symlinks fail closed on both sides and never get followed
                // into a create; the target keeps its own valid header.
                #[cfg(unix)]
                if state == FileState::Symlink {
                    assert!(
                        SharedAudioBuffer::open(&path).is_err(),
                        "{case}: HAL open must reject symlink"
                    );
                    assert!(
                        daemon_start(&path).is_err(),
                        "{case}: daemon start must reject symlink"
                    );
                    let target = case_dir.join("real-audio.shm");
                    let untouched = SharedAudioBuffer::open(&target).expect("target untouched");
                    assert_eq!(untouched.sample_rate(), GEOMETRY_48K.0, "{case}");
                    continue;
                }

                // HAL-first: the open-only side must never create the file.
                let mut hal_early: Option<SharedAudioBuffer> = None;
                if order == StartOrder::HalFirst {
                    let existed_before = path.exists();
                    hal_early = SharedAudioBuffer::open(&path).ok();
                    assert_eq!(
                        path.exists(),
                        existed_before,
                        "{case}: HAL open must not create the exchange file"
                    );
                    if state == FileState::Absent {
                        assert!(
                            hal_early.is_none(),
                            "{case}: HAL-first open of absent file must fail"
                        );
                    }
                }

                let mut daemon = daemon_start(&path)
                    .unwrap_or_else(|error| panic!("{case}: daemon start failed: {error}"));

                let mut hal = match hal_early {
                    Some(buffer) => buffer,
                    None => SharedAudioBuffer::open(&path).unwrap_or_else(|error| {
                        panic!("{case}: HAL open after daemon start failed: {error}")
                    }),
                };

                if order == StartOrder::DaemonRestart {
                    drop(daemon);
                    daemon = daemon_start(&path)
                        .unwrap_or_else(|error| panic!("{case}: daemon restart failed: {error}"));
                    assert!(
                        hal.backing_file_is_current(),
                        "{case}: daemon restart must not orphan the HAL mapping"
                    );
                }

                // Final geometry: adoptable files keep theirs; everything
                // else converges to the daemon default.
                let expected = state.expected_geometry();
                assert_eq!(
                    (
                        daemon.sample_rate(),
                        daemon.buffer_frames(),
                        daemon.channel_count()
                    ),
                    expected,
                    "{case}: final daemon geometry"
                );
                assert_eq!(
                    (hal.sample_rate(), hal.buffer_frames(), hal.channel_count()),
                    expected,
                    "{case}: final HAL geometry"
                );

                // Header validity and flag hygiene after any start order.
                let header = daemon.header();
                assert_eq!(
                    header.magic.load(Ordering::Acquire),
                    SHARED_MEMORY_MAGIC,
                    "{case}"
                );
                assert_eq!(
                    header.version.load(Ordering::Acquire),
                    SHARED_MEMORY_VERSION,
                    "{case}"
                );
                if state == FileState::StaleFlags {
                    // driver_ready is HAL-owned and survives daemon restarts.
                    assert!(
                        daemon.driver_ready(),
                        "{case}: driver_ready must be preserved"
                    );
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = fs::metadata(&path)
                        .expect("stat audio.shm")
                        .permissions()
                        .mode()
                        & 0o777;
                    assert_eq!(mode, 0o600, "{case}: exchange file must end owner-only");
                }

                // Crypto state survives only when the header was preserved.
                let preserved = state.is_adoptable();
                if preserved && key != KeyState::Plaintext {
                    assert!(
                        daemon.is_encrypted(),
                        "{case}: encryption flag must be preserved"
                    );
                    let expected_fingerprint = if key == KeyState::FlaggedNoFingerprint {
                        [0; 8]
                    } else {
                        *AudioCipher::new(&KEY_A).fingerprint()
                    };
                    assert_eq!(daemon.key_fingerprint(), expected_fingerprint, "{case}");
                } else {
                    assert!(
                        !daemon.is_encrypted(),
                        "{case}: re-init must reset encryption off"
                    );
                }

                assert_transport_works(&mut daemon, &mut hal, key, preserved, &case);
            }
        }
    }
}

#[test]
fn startup_matrix_config_negotiation_round_trips() {
    // HAL- and daemon-initiated requests across the supported rates,
    // including the cold-start 48k -> 96k request coreaudiod posts when
    // its nominal rate outlives the previous daemon session.
    let dir = tempdir().expect("create negotiation temp dir");
    for (from, to) in [
        (48_000, 96_000),
        (96_000, 48_000),
        (48_000, 192_000),
        (96_000, 44_100),
        (44_100, 176_400),
    ] {
        for source in [1u32, 2] {
            let case = format!("config {from}->{to} source={source}");
            let case_dir = dir.path().join(format!("neg-{from}-{to}-{source}"));
            fs::create_dir_all(&case_dir).expect("case dir");
            let path = case_dir.join("audio.shm");
            create_valid(&path, (from, 512, 2));

            let mut daemon = daemon_start(&path).expect("daemon start");
            let mut hal = SharedAudioBuffer::open(&path).expect("HAL open");

            hal.request_config_change(to, 512, 2, source);
            assert!(daemon.config_changed(), "{case}: request must be visible");
            assert_eq!(daemon.requested_sample_rate(), to, "{case}");
            assert_eq!(daemon.config_source(), source, "{case}");

            daemon.reconfigure_quiesced(Some(to), Some(512), Some(2));
            daemon.acknowledge_config_change(to, 512, 1, 0);

            assert!(!hal.config_changed(), "{case}: ack must clear the request");
            assert_eq!(hal.actual_sample_rate(), to, "{case}");
            assert_eq!(hal.sample_rate(), to, "{case}");
            assert_eq!(daemon.sample_rate(), to, "{case}");

            let samples = sample_block(32, 2);
            let mut out = vec![0.0f32; samples.len()];
            assert_eq!(
                hal.write_audio(&samples),
                32,
                "{case}: post-negotiation write"
            );
            assert_eq!(
                daemon.read_audio(&mut out),
                32,
                "{case}: post-negotiation read"
            );
            assert_eq!(out, samples, "{case}: post-negotiation roundtrip");
        }
    }
}

#[test]
fn startup_matrix_plaintext_is_never_accepted_as_encrypted() {
    let dir = tempdir().expect("create temp dir");
    let path = dir.path().join("audio.shm");
    create_valid(&path, GEOMETRY_48K);
    apply_key_state(&path, KeyState::Matching);

    let daemon = daemon_start(&path).expect("daemon start");
    let mut hal = SharedAudioBuffer::open(&path).expect("HAL open");
    assert!(daemon.is_encrypted());

    // A plaintext writer (stale HAL build, attacker, misroute) must not
    // smuggle frames past an encrypted transport.
    let samples = sample_block(64, 2);
    let _ = hal.write_audio(&samples);
    let mut out = vec![0.0f32; samples.len()];
    let read = daemon.read_audio_encrypted(&mut out, &AudioCipher::new(&KEY_A));
    assert_eq!(
        read, 0,
        "plaintext must not be accepted on an encrypted transport"
    );
    assert_ne!(out, samples);
}
