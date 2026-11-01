//! Compare prepared geometry and key while owning the ring write transaction.

use super::{CONFIGURING_WRITE_COMMIT, SharedAudioBuffer};
use crate::encryption::AudioCipher;
use crate::shared_memory::misc::fingerprints_equal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::shared_memory) enum PreparedWriteRefusal {
    Unavailable,
    FormatChanged,
    KeyMismatch,
}

struct WriteCommit<'a> {
    shared: &'a mut SharedAudioBuffer,
}
impl Drop for WriteCommit<'_> {
    fn drop(&mut self) {
        self.shared.release_io_commit(CONFIGURING_WRITE_COMMIT);
    }
}

impl SharedAudioBuffer {
    pub(in crate::shared_memory) fn write_prepared(
        &mut self,
        samples: &[f32],
        expected: (u32, u32, u32),
        cipher: Option<&AudioCipher>,
        ciphertext: &mut Vec<u8>,
        encrypted: &mut Vec<f32>,
    ) -> Result<usize, PreparedWriteRefusal> {
        if !self.try_acquire_io_commit(CONFIGURING_WRITE_COMMIT) {
            return Err(PreparedWriteRefusal::Unavailable);
        }
        let guard = WriteCommit { shared: self };
        if !guard.shared.driver_ready() {
            return Err(PreparedWriteRefusal::Unavailable);
        }
        let actual = (
            guard.shared.sample_rate(),
            guard.shared.channel_count(),
            guard.shared.buffer_frames(),
        );
        if expected.0 == 0 || expected.1 == 0 || expected.2 == 0 || actual != expected {
            return Err(PreparedWriteRefusal::FormatChanged);
        }
        // Owners using the quiesced protocol cannot change geometry or
        // encryption while this commit is held. Keep key validation in the same
        // section as encryption, the ring copy and cursor publication.
        if guard.shared.is_encrypted() {
            let cipher = cipher
                .filter(|c| fingerprints_equal(c.fingerprint(), &guard.shared.key_fingerprint()))
                .ok_or(PreparedWriteRefusal::KeyMismatch)?;
            Ok(guard
                .shared
                .write_audio_encrypted_under_commit(samples, cipher, ciphertext, encrypted))
        } else {
            Ok(guard.shared.write_audio_under_commit(samples))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn fixture() -> (tempfile::TempDir, SharedAudioBuffer) {
        let root = std::path::Path::new("/Volumes/home_tmp/tmp");
        let root = if root.is_dir() {
            root.to_path_buf()
        } else {
            std::env::temp_dir()
        };
        let directory = tempfile::tempdir_in(root).unwrap();
        let buffer =
            SharedAudioBuffer::create_or_open(directory.path().join("audio.shm"), 48_000, 64, 2)
                .unwrap();
        buffer.header().driver_ready.store(1, Ordering::Release);
        (directory, buffer)
    }
    fn write(
        buffer: &mut SharedAudioBuffer,
        format: (u32, u32, u32),
        cipher: Option<&AudioCipher>,
    ) -> Result<usize, PreparedWriteRefusal> {
        buffer.write_prepared(
            &[0.125, 0.25],
            format,
            cipher,
            &mut Vec::with_capacity(4096),
            &mut Vec::with_capacity(1024),
        )
    }

    #[test]
    fn prepared_geometry_and_key_refusals_do_not_publish_or_retain_the_guard() {
        let (_directory, mut buffer) = fixture();
        for format in [
            (96_000, 2, 64),
            (48_000, 1, 64),
            (48_000, 2, 128),
            (0, 2, 64),
        ] {
            let before = buffer.header().write_position.load(Ordering::Acquire);
            assert_eq!(
                write(&mut buffer, format, None),
                Err(PreparedWriteRefusal::FormatChanged)
            );
            assert_eq!(
                buffer.header().write_position.load(Ordering::Acquire),
                before
            );
            assert_eq!(
                buffer.header().configuring.load(Ordering::Acquire) & CONFIGURING_WRITE_COMMIT,
                0
            );
        }
        let a = AudioCipher::new(&[0x11; 32]);
        let b = AudioCipher::new(&[0x22; 32]);
        buffer.set_encrypted(true);
        buffer.set_key_fingerprint(*b.fingerprint());
        let before = buffer.header().write_position.load(Ordering::Acquire);
        assert_eq!(
            write(&mut buffer, (48_000, 2, 64), Some(&a)),
            Err(PreparedWriteRefusal::KeyMismatch)
        );
        assert_eq!(
            buffer.header().write_position.load(Ordering::Acquire),
            before
        );
        assert_eq!(write(&mut buffer, (48_000, 2, 64), Some(&b)), Ok(1));
        buffer.set_encrypted(false);
        buffer.flush_audio();
        assert_eq!(write(&mut buffer, (48_000, 2, 64), None), Ok(1));
        let mut out = [0.0; 2];
        assert_eq!(buffer.read_audio(&mut out), 1);
        assert_eq!(out, [0.125, 0.25]);
    }

    #[test]
    fn prepared_write_guard_excludes_quiesced_geometry_publication() {
        let (directory, mut buffer) = fixture();
        let mut control = SharedAudioBuffer::open(directory.path().join("audio.shm")).unwrap();
        assert!(buffer.try_acquire_io_commit(CONFIGURING_WRITE_COMMIT));
        {
            let guard = WriteCommit {
                shared: &mut buffer,
            };
            control.set_sample_rate(96_000);
            assert_eq!(guard.shared.sample_rate(), 48_000);
        }
        control.set_sample_rate(96_000);
        assert_eq!(buffer.sample_rate(), 96_000);
        assert_eq!(
            write(&mut buffer, (48_000, 2, 64), None),
            Err(PreparedWriteRefusal::FormatChanged)
        );
    }

    #[test]
    fn concurrent_control_never_receives_prepared_48k_audio_in_a_96k_epoch() {
        use std::sync::{Arc, atomic::AtomicUsize};
        use std::time::{Duration, Instant};

        let (directory, mut writer) = fixture();
        let path = directory.path().join("audio.shm");
        let reader = SharedAudioBuffer::open(&path).unwrap();
        let mut control = SharedAudioBuffer::open(&path).unwrap();
        control.set_sample_rate(96_000);
        assert_eq!(
            write(&mut writer, (48_000, 2, 64), None),
            Err(PreparedWriteRefusal::FormatChanged)
        );
        // A request can be refused while an I/O commit is held. Require an
        // actual read observation of each epoch before requesting the next one.
        let epochs = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
        let control_epochs = Arc::clone(&epochs);
        let deadline = Instant::now() + Duration::from_secs(10);
        let worker = std::thread::spawn(move || {
            for epoch in 1..=512 {
                let rate = if epoch % 2 == 1 { 48_000 } else { 96_000 };
                control_epochs.0.store(epoch, Ordering::Release);
                while control_epochs.1.load(Ordering::Acquire) < epoch {
                    assert!(Instant::now() < deadline, "epoch {epoch} was not observed");
                    if control.sample_rate() != rate {
                        control.set_sample_rate(rate);
                    }
                    std::thread::yield_now();
                }
            }
        });
        let mut ciphertext = Vec::with_capacity(4096);
        let mut encrypted = Vec::with_capacity(1024);
        let mut observed_96k = 0;
        let mut observations = 0;
        while observations < 20_000 || epochs.1.load(Ordering::Acquire) < 512 {
            assert!(Instant::now() < deadline, "epoch observation timed out");
            observations += 1;
            let epoch = epochs.0.load(Ordering::Acquire);
            let _ = writer.write_prepared(
                &[0.125, 0.25],
                (48_000, 2, 64),
                None,
                &mut ciphertext,
                &mut encrypted,
            );
            if let Some(mut commit) = reader.try_read_commit() {
                let rate = commit.identity().sample_rate;
                let mut out = [0.0; 256];
                let frames = commit.read_audio(&mut out);
                if rate == 96_000 {
                    assert_eq!(frames, 0);
                    observed_96k += 1;
                } else {
                    assert_eq!(rate, 48_000);
                    for frame in out[..frames * 2].as_chunks::<2>().0 {
                        assert_eq!(*frame, [0.125, 0.25]);
                    }
                }
                let requested_rate = if epoch % 2 == 1 { 48_000 } else { 96_000 };
                if epoch != 0 && rate == requested_rate {
                    epochs.1.store(epoch, Ordering::Release);
                }
            }
            std::thread::yield_now();
        }
        worker.join().unwrap();
        assert_eq!(epochs.1.load(Ordering::Acquire), 512);
        assert!(observed_96k >= 256);
        println!(
            "prepared_write_control observations={observations} control_epochs=512 rate96_empty_observations={observed_96k}"
        );
    }
}
