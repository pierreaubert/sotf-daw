//! Control-only encryption publication and compatibility-setter regressions.

// Rust guideline compliant 2026-02-21
use super::{CONFIGURING_RECONFIGURE, SharedAudioBuffer};
use std::io;

struct Publication<'a>(&'a SharedAudioBuffer);

impl Drop for Publication<'_> {
    fn drop(&mut self) {
        self.0.release_io_commit(CONFIGURING_RECONFIGURE);
    }
}

impl SharedAudioBuffer {
    /// Run a control publication after excluding all audio commits.
    pub(super) fn with_encryption_update(
        &self,
        publish: impl FnOnce(&Self) -> io::Result<()>,
    ) -> io::Result<()> {
        #[cfg(test)]
        before_publication();
        if !self.begin_reconfiguration() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "HAL encryption publication is busy",
            ));
        }
        let _publication = Publication(self);
        publish(self)
    }
}

#[cfg(test)]
pub(super) fn before_publication() {
    tests::pause_before_publication();
}

#[cfg(test)]
mod tests {
    use super::super::{CONFIGURING_WRITE_COMMIT, SharedAudioBuffer};
    use std::cell::RefCell;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::Duration;

    thread_local! {
        static PAUSE: RefCell<Option<(Sender<()>, Receiver<()>)>> = const { RefCell::new(None) };
    }

    pub(super) fn pause_before_publication() {
        if let Some((ready, resume)) = PAUSE.with(|slot| slot.borrow_mut().take()) {
            ready.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }

    #[derive(Clone, Copy)]
    enum Update {
        Mode,
        Key,
        Flush,
    }

    fn apply(buffer: &SharedAudioBuffer, update: Update) {
        match update {
            Update::Mode => buffer.set_encrypted(true),
            Update::Key => buffer.set_key_fingerprint([0x22; 8]),
            Update::Flush => buffer.flush_audio(),
        }
    }

    fn fixture() -> (tempfile::TempDir, SharedAudioBuffer) {
        let preferred = std::path::Path::new("/Volumes/home_tmp/tmp");
        let directory = if preferred.is_dir() {
            tempfile::tempdir_in(preferred)
        } else {
            tempfile::tempdir()
        }
        .unwrap();
        let mut buffer =
            SharedAudioBuffer::create_or_open(directory.path().join("audio.shm"), 48_000, 64, 2)
                .unwrap();
        buffer.set_key_fingerprint([0x11; 8]);
        assert_eq!(buffer.write_audio(&[0.125, 0.25, 0.375, 0.5]), 2);
        (directory, buffer)
    }

    #[test]
    fn compatibility_encryption_publications_respect_inflight_audio_io() {
        let mut failures = 0;
        for update in [Update::Mode, Update::Key, Update::Flush] {
            let (_directory, buffer) = fixture();
            let before = (
                buffer.is_encrypted(),
                buffer.key_fingerprint(),
                buffer.header().read_position.load(Ordering::Acquire),
            );
            assert!(buffer.try_acquire_io_commit(CONFIGURING_WRITE_COMMIT));
            apply(&buffer, update);
            let after = (
                buffer.is_encrypted(),
                buffer.key_fingerprint(),
                buffer.header().read_position.load(Ordering::Acquire),
            );
            buffer.release_io_commit(CONFIGURING_WRITE_COMMIT);
            failures += usize::from(before != after);
            apply(&buffer, update);
            match update {
                Update::Mode => assert!(buffer.is_encrypted()),
                Update::Key => assert_eq!(buffer.key_fingerprint(), [0x22; 8]),
                Update::Flush => assert_eq!(
                    buffer.header().read_position.load(Ordering::Acquire),
                    buffer.header().write_position.load(Ordering::Acquire)
                ),
            }
        }
        println!("encryption_io_publication cases=3 failures={failures}");
        assert_eq!(
            failures, 0,
            "control publication must wait for or refuse an active audio commit"
        );
    }

    #[test]
    fn compatibility_field_updates_and_flush_never_restore_an_untouched_stale_field() {
        let mut failures = 0;
        for update in [Update::Mode, Update::Key, Update::Flush] {
            let (directory, buffer) = fixture();
            let path = directory.path().join("audio.shm");
            let (ready_tx, ready_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let buffer = SharedAudioBuffer::open(path).unwrap();
                PAUSE.with(|slot| *slot.borrow_mut() = Some((ready_tx, resume_rx)));
                apply(&buffer, update);
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            match update {
                Update::Mode => buffer.set_key_fingerprint([0x22; 8]),
                Update::Key => buffer.set_encrypted(true),
                Update::Flush => {
                    buffer.set_encrypted(true);
                    buffer.set_key_fingerprint([0x22; 8]);
                }
            }
            resume_tx.send(()).unwrap();
            worker.join().unwrap();
            failures +=
                usize::from(!buffer.is_encrypted() || buffer.key_fingerprint() != [0x22; 8]);
        }
        println!("encryption_field_publication cases=3 failures={failures}");
        assert_eq!(
            failures, 0,
            "a setter or flush must not restore fields read before publication ownership"
        );
    }
    #[test]
    fn publication_releases_control_ownership_after_error_and_unwind() {
        let (_directory, buffer) = fixture();
        assert!(
            buffer
                .with_encryption_update(|_| Err(std::io::Error::other("injected")))
                .is_err()
        );
        assert_eq!(buffer.header().configuring.load(Ordering::Acquire), 0);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = buffer.with_encryption_update(|_| panic!("injected control unwind"));
        }));
        assert!(result.is_err());
        assert_eq!(buffer.header().configuring.load(Ordering::Acquire), 0);
        buffer.set_encrypted(true);
        assert!(buffer.is_encrypted());
    }
}
