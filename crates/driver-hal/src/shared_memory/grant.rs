use std::io;
use std::path::Path;

// Rust guideline compliant 2026-02-21

/// Grant the sandboxed `coreaudiod` process access to a shared-memory path.
///
/// Directories receive `search,readattr` so the HAL plugin can traverse the
/// per-user runtime directory; the audio file itself receives
/// `read,write,readattr,writeattr` for the ring-buffer protocol.
///
/// Failures are returned to the caller instead of only logged: without this
/// grant the HAL transport cannot operate, and warn-and-continue degrades
/// silently into undecodable audio. Callers surface the error through driver
/// initialization so the failure is explicit.
#[cfg(target_os = "macos")]
pub(super) fn grant_coreaudiod_shared_memory_access(path: &Path) -> io::Result<()> {
    let acl = if path.is_dir() {
        "_coreaudiod allow search,readattr"
    } else {
        "_coreaudiod allow read,write,readattr,writeattr"
    };

    let status = std::process::Command::new("/bin/chmod")
        .arg("+a")
        .arg(acl)
        .arg(path)
        .status()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to grant _coreaudiod shared-memory access on {}: {error}",
                    path.display()
                ),
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to grant _coreaudiod shared-memory access on {}: chmod exited with {status}",
            path.display()
        )))
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn grant_coreaudiod_shared_memory_access(_path: &Path) -> io::Result<()> {
    Ok(())
}
