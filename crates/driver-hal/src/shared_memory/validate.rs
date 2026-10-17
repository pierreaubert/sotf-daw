#[cfg(unix)]
use super::current::current_uid;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;

pub(super) fn open_shared_memory_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
        options.mode(0o600);
    }

    let file = options.open(path)?;
    // The create path owns this file: repair a legacy loose mode left by an
    // older build before validating, instead of wedging every daemon start
    // until the user deletes the file by hand. Ownership and symlink checks
    // in validation still fail closed. The open-only path used by readers
    // keeps rejecting loose modes until the daemon has repaired them.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    validate_shared_memory_file(&file, path)?;
    Ok(file)
}

pub(super) fn open_existing_shared_memory_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }

    let file = options.open(path)?;
    validate_shared_memory_file(&file, path)?;
    Ok(file)
}

#[cfg(unix)]
pub(super) fn validate_shared_memory_file(file: &std::fs::File, path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a regular file", path.display()),
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is owned by uid {}, expected {}",
                path.display(),
                metadata.uid(),
                current_uid()
            ),
        ));
    }
    // The daemon creates the file mode 0600 and the parent directory mode
    // 0700, but an existing file is opened without a mode change: reject
    // group/other readability left behind by older builds or same-user
    // mistakes so captured system audio is never world-readable.
    // macOS ACL grants for `_coreaudiod` do not alter these mode bits.
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 || mode & 0o600 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} has insecure permissions 0o{mode:o} (expected owner-only 0600)",
                path.display(),
            ),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn validate_shared_memory_file(_file: &std::fs::File, _path: &Path) -> io::Result<()> {
    Ok(())
}
