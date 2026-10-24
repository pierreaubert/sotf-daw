//! Darwin shared memory with an immutable extent and authenticated FD handoff.
//!
//! `audio.shm` is a 32-byte rendezvous record, never an audio mmap. The audio
//! object is allocated once with `shm_open`, immediately unlinked, and passed
//! through SCM_RIGHTS. Darwin rejects every later truncate of that object.

// Rust guideline compliant 2026-02-21

use super::current::current_uid;
use super::grant::grant_coreaudiod_shared_memory_access;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::net::{
    AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
    SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketAddrUnix, SocketType, connect,
    recvmsg, sendmsg, socket,
};
use std::fs::{File, Metadata};
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(super) const RECORD_LENGTH: usize = 32;
const RECORD_MAGIC: &[u8; 8] = b"SOTFMM08";
const MAX_REGION_LENGTH: usize = 64 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rendezvous {
    pub length: usize,
    nonce: [u8; 16],
}

impl Rendezvous {
    pub fn new(length: usize) -> io::Result<Self> {
        check_length(length)?;
        Ok(Self {
            length,
            nonce: rand::random(),
        })
    }

    pub fn encode(self) -> [u8; RECORD_LENGTH] {
        let mut bytes = [0; RECORD_LENGTH];
        bytes[..8].copy_from_slice(RECORD_MAGIC);
        bytes[8..16].copy_from_slice(&(self.length as u64).to_le_bytes());
        bytes[16..].copy_from_slice(&self.nonce);
        bytes
    }

    pub fn read(file: &mut File) -> io::Result<Self> {
        if file.metadata()?.len() != RECORD_LENGTH as u64 {
            return Err(invalid("unsupported HAL rendezvous length"));
        }
        let mut bytes = [0; RECORD_LENGTH];
        file.read_exact(&mut bytes)?;
        if &bytes[..8] != RECORD_MAGIC {
            return Err(invalid("unsupported HAL rendezvous format"));
        }
        let length = usize::try_from(u64::from_le_bytes(bytes[8..16].try_into().unwrap()))
            .map_err(|_| invalid("HAL region length does not fit this platform"))?;
        check_length(length)?;
        Ok(Self {
            length,
            nonce: bytes[16..].try_into().unwrap(),
        })
    }

    pub fn socket_path(self, path: &Path) -> io::Result<PathBuf> {
        let parent = path
            .parent()
            .ok_or_else(|| invalid("HAL rendezvous requires a parent"))?;
        let suffix = u64::from_be_bytes(self.nonce[..8].try_into().unwrap());
        Ok(parent.join(format!(".fd-{suffix:016x}")))
    }
}

fn check_length(length: usize) -> io::Result<()> {
    if !(192..=MAX_REGION_LENGTH).contains(&length) {
        return Err(invalid("HAL memory length is outside transport bounds"));
    }
    Ok(())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Allocate once, then remove the name before publishing any descriptor.
pub(super) fn create_region(length: usize) -> io::Result<File> {
    check_length(length)?;
    let name = format!("/sotf-{:024x}", rand::random::<u128>() & ((1 << 96) - 1));
    let fd = rustix::shm::open(
        name.as_str(),
        rustix::shm::OFlags::CREATE | rustix::shm::OFlags::EXCL | rustix::shm::OFlags::RDWR,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    rustix::shm::unlink(name.as_str())?;
    rustix::fs::ftruncate(&fd, length as u64)?;
    let file = File::from(fd);
    validate_region(&file, length, current_uid())?;
    Ok(file)
}

/// Refuse ordinary files before mmap, including a malicious broker's substitute.
pub(super) fn validate_region(file: &File, length: usize, owner: u32) -> io::Result<()> {
    check_length(length)?;
    let metadata = file.metadata()?;
    // XNU's POSIX shm fstat has no vnode file-type bits. Ordinary files,
    // sockets and device nodes cannot pass this test by chmod'ing their mode.
    if metadata.mode() & u32::from(libc::S_IFMT) != 0
        || metadata.uid() != owner
        || metadata.len() < length as u64
        || metadata.len() > (MAX_REGION_LENGTH + 65_536) as u64
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "received descriptor is not an owner-allocated Darwin shared-memory object",
        ));
    }
    // An allocated Darwin shm extent rejects even a same-size truncate. This
    // also refuses an unallocated object rather than allocating peer storage.
    if rustix::fs::ftruncate(file, metadata.len()) != Err(rustix::io::Errno::INVAL) {
        return Err(invalid("HAL backing extent is not immutable"));
    }
    Ok(())
}

fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: getpeereid writes two initialized scalar outputs for a valid
    // borrowed socket. It retains no pointer and does not take FD ownership.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

fn coreaudiod_uid() -> io::Result<u32> {
    let mut record = MaybeUninit::<libc::passwd>::uninit();
    let mut storage = [0u8; 16_384];
    let mut result = std::ptr::null_mut();
    // SAFETY: the reentrant lookup receives writable output/scratch buffers
    // of their declared sizes and a static NUL-terminated account name.
    let status = unsafe {
        libc::getpwnam_r(
            c"_coreaudiod".as_ptr(),
            record.as_mut_ptr(),
            storage.as_mut_ptr().cast(),
            storage.len(),
            &mut result,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status));
    }
    if result.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "_coreaudiod account is unavailable",
        ));
    }
    // SAFETY: a successful non-null lookup initialized the caller's record.
    Ok(unsafe { record.assume_init() }.pw_uid)
}

fn read_frame(stream: &mut UnixStream, bytes: &mut [u8]) -> io::Result<()> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut offset = 0;
    while offset < bytes.len() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "HAL descriptor request timed out")
            })?;
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(&mut bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete HAL descriptor request",
                ));
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "HAL descriptor request timed out",
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn send_region(stream: &mut UnixStream, file: &File, record: Rendezvous) -> io::Result<()> {
    let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut storage);
    let descriptors = [file.as_fd()];
    if !control.push(SendAncillaryMessage::ScmRights(&descriptors)) {
        return Err(invalid("HAL descriptor message has no ancillary capacity"));
    }
    let bytes = record.encode();
    let sent = sendmsg(
        &*stream,
        &[IoSlice::new(&bytes)],
        &mut control,
        SendFlags::empty(),
    )?;
    if sent == 0 {
        return Err(io::ErrorKind::WriteZero.into());
    }
    stream.write_all(&bytes[sent..])?;
    Ok(())
}

fn connect_bounded(path: &Path) -> io::Result<UnixStream> {
    let fd = socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
    rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
    rustix::io::ioctl_fionbio(&fd, true)?;
    rustix::net::sockopt::set_socket_nosigpipe(&fd, true)?;
    let address = SocketAddrUnix::new(path)?;
    match connect(&fd, &address) {
        Ok(()) => {}
        Err(rustix::io::Errno::INPROGRESS | rustix::io::Errno::AGAIN) => {
            let mut descriptors = [PollFd::new(&fd, PollFlags::OUT)];
            let timeout = Timespec {
                tv_sec: 0,
                tv_nsec: 250_000_000,
            };
            if poll(&mut descriptors, Some(&timeout))? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "HAL descriptor connection timed out",
                ));
            }
        }
        Err(error) => return Err(error.into()),
    }
    let stream = UnixStream::from(fd);
    if let Some(error) = stream.take_error()? {
        return Err(error);
    }
    stream.peer_addr()?;
    stream.set_nonblocking(false)?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    Ok(stream)
}

pub(super) fn receive_region(path: &Path, record: Rendezvous, owner: u32) -> io::Result<File> {
    receive_from_socket(&record.socket_path(path)?, record, owner)
}

fn receive_from_socket(path: &Path, record: Rendezvous, owner: u32) -> io::Result<File> {
    let mut stream = connect_bounded(path)?;
    if peer_uid(&stream)? != owner {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "HAL descriptor broker has the wrong owner",
        ));
    }
    let expected = record.encode();
    stream.write_all(&expected)?;
    let mut bytes = [0; RECORD_LENGTH];
    let mut storage = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(4))];
    let mut control = RecvAncillaryBuffer::new(&mut storage);
    let message = recvmsg(
        &stream,
        &mut [IoSliceMut::new(&mut bytes)],
        &mut control,
        RecvFlags::WAITALL,
    )?;
    let mut descriptor = None;
    let mut malformed = message.bytes != RECORD_LENGTH
        || message.flags.contains(ReturnFlags::CTRUNC)
        || bytes != expected;
    for item in control.drain() {
        match item {
            RecvAncillaryMessage::ScmRights(descriptors) => {
                for fd in descriptors {
                    if descriptor.is_some() {
                        malformed = true;
                    } else {
                        descriptor = Some(fd);
                    }
                }
            }
            _ => malformed = true,
        }
    }
    // OwnedFd and the ancillary drain close every received descriptor on error.
    if malformed {
        return Err(invalid("invalid HAL descriptor response"));
    }
    let fd = descriptor.ok_or_else(|| invalid("HAL descriptor response omitted memory"))?;
    rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
    let file = File::from(fd);
    validate_region(&file, record.length, owner)?;
    Ok(file)
}

/// One control-only server for one immutable object and one nonce-named endpoint.
pub(super) struct DescriptorServer {
    path: PathBuf,
    identity: (u64, u64),
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DescriptorServer {
    pub fn is_running(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    pub fn start(path: &Path, file: File, record: Rendezvous) -> io::Result<Self> {
        let coreaudiod = coreaudiod_uid()?;
        let owner = current_uid();
        let path = record.socket_path(path)?;
        let listener = UnixListener::bind(&path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        let stop = Arc::new(AtomicBool::new(false));
        let mut server = Self {
            path,
            identity: identity(&metadata),
            stop: Arc::clone(&stop),
            worker: None,
        };
        std::fs::set_permissions(&server.path, std::fs::Permissions::from_mode(0o600))?;
        grant_coreaudiod_shared_memory_access(&server.path)?;
        listener.set_nonblocking(true)?;
        server.worker = Some(
            std::thread::Builder::new()
                .name("hal-descriptors".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        match listener.accept() {
                            Ok((mut stream, _)) => {
                                let result = (|| -> io::Result<()> {
                                    let uid = peer_uid(&stream)?;
                                    if uid != owner && uid != coreaudiod && uid != 0 {
                                        return Err(io::ErrorKind::PermissionDenied.into());
                                    }
                                    stream.set_nonblocking(false)?;
                                    rustix::net::sockopt::set_socket_nosigpipe(&stream, true)?;
                                    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
                                    let mut request = [0; RECORD_LENGTH];
                                    read_frame(&mut stream, &mut request)?;
                                    if request != record.encode() || stop.load(Ordering::Acquire) {
                                        return Err(invalid("obsolete HAL descriptor request"));
                                    }
                                    send_region(&mut stream, &file, record)
                                })();
                                if let Err(error) = result {
                                    log::debug!("HAL descriptor handoff rejected: {error}");
                                }
                            }
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                std::thread::park_timeout(Duration::from_millis(10));
                            }
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                            Err(error) => {
                                log::warn!("HAL descriptor listener stopped: {error}");
                                break;
                            }
                        }
                    }
                })?,
        );
        Ok(server)
    }
}

fn identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

impl Drop for DescriptorServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if worker.join().is_err() {
                log::warn!("HAL descriptor worker panicked during shutdown");
            }
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && identity(&metadata) == self.identity
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocated_darwin_object_cannot_be_resized_and_remains_mapped() {
        let file = create_region(4096).unwrap();
        // SAFETY: create_region validated an allocated, immutable Darwin shm
        // extent. No file path can truncate this object after allocation.
        let mut mapping = unsafe {
            memmap2::MmapOptions::new()
                .len(4096)
                .map_mut(&file)
                .unwrap()
        };
        for length in [0, 1, 4096, file.metadata().unwrap().len(), 1_048_576] {
            assert_eq!(
                rustix::fs::ftruncate(&file, length),
                Err(rustix::io::Errno::INVAL)
            );
            mapping[0] = 17;
            mapping[4095] = 29;
            assert_eq!((mapping[0], mapping[4095]), (17, 29));
        }
    }

    #[test]
    fn broker_handoff_preserves_object_and_rejects_wrong_nonce() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audio.shm");
        let file = create_region(4096).unwrap();
        let record = Rendezvous::new(4096).unwrap();
        let server = DescriptorServer::start(&path, file.try_clone().unwrap(), record).unwrap();
        let received = receive_region(&path, record, current_uid()).unwrap();
        assert!(
            rustix::io::fcntl_getfd(&received)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
        assert_eq!(
            received.metadata().unwrap().len(),
            file.metadata().unwrap().len()
        );
        let wrong = Rendezvous::new(4096).unwrap();
        assert!(receive_from_socket(&server.path, wrong, current_uid()).is_err());
        assert!(receive_region(&path, record, current_uid()).is_ok());
        drop(server);
        assert!(!record.socket_path(&path).unwrap().exists());
        validate_region(&received, 4096, current_uid()).unwrap();
    }

    #[test]
    fn a_same_owner_broker_cannot_substitute_a_truncatable_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audio.shm");
        let record = Rendezvous::new(4096).unwrap();
        let file = tempfile::tempfile().unwrap();
        file.set_len(4096).unwrap();
        let listener = UnixListener::bind(record.socket_path(&path).unwrap()).unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; RECORD_LENGTH];
            read_frame(&mut stream, &mut request).unwrap();
            send_region(&mut stream, &file, record).unwrap();
        });
        let error = receive_region(&path, record, current_uid()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        worker.join().unwrap();
    }

    #[test]
    fn replacement_keeps_pinned_pages_and_cannot_remove_the_new_endpoint() {
        use super::super::shared_audio_buffer::SharedAudioBuffer;
        let directory = tempfile::Builder::new()
            .prefix("fd-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = directory.path().join("audio.shm");
        let first =
            SharedAudioBuffer::create_replacement_with_max_geometry(&path, 48_000, 64, 2, 64, 2)
                .unwrap();
        first.set_output_volume(0.375, true);
        let mut pinned = SharedAudioBuffer::open_brokered(&path).unwrap();
        let second =
            SharedAudioBuffer::create_replacement_with_max_geometry(&path, 48_000, 64, 2, 64, 2)
                .unwrap();
        assert!(!first.backing_file_is_current());
        assert!(second.backing_file_is_current());
        assert_eq!(second.output_volume(), first.output_volume());
        assert!(!second.driver_ready());
        drop(first);
        assert!(SharedAudioBuffer::open_brokered(&path).is_ok());
        // Deleting/truncating a rendezvous cannot revoke either mapped object.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(0)
            .unwrap();
        assert!(!second.backing_file_is_current());
        assert!(SharedAudioBuffer::open_brokered(&path).is_err());
        assert_eq!(pinned.write_audio(&[0.25; 32]), 16);
        let mut output = [0.; 32];
        assert_eq!(pinned.read_audio(&mut output), 16);
        assert_eq!(output, [0.25; 32]);
    }

    #[test]
    fn production_attachment_rejects_a_legacy_fixture() {
        use super::super::shared_audio_buffer::SharedAudioBuffer;
        let fixture = tempfile::NamedTempFile::new().unwrap();
        let _mapping = SharedAudioBuffer::create_or_open(fixture.path(), 48_000, 64, 2).unwrap();
        assert!(SharedAudioBuffer::open_brokered(fixture.path()).is_err());
    }

    #[test]
    fn partial_requests_have_one_absolute_deadline() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(&[1]).unwrap();
        let start = Instant::now();
        let mut request = [0; RECORD_LENGTH];
        assert_eq!(
            read_frame(&mut server, &mut request).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
