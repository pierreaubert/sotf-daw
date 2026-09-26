//! Behavioral test for the Linux seccomp child-process denial.
//!
//! The filter is installed only in a forked child, never in the test runner:
//! installing it in-process would sandbox the harness itself.

use super::super::seccomp_child_process::deny_child_processes;

#[test]
fn seccomp_filter_denies_processes_but_allows_threads() {
    unsafe {
        let pid = libc::fork();
        assert!(pid >= 0, "test setup fork failed");
        if pid == 0 {
            libc::_exit(filter_probe());
        }
        let mut status = 0;
        assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
        assert!(
            libc::WIFEXITED(status),
            "filter probe child did not exit normally"
        );
        let code = libc::WEXITSTATUS(status);
        assert_eq!(code, 0, "filter probe failed with code {code}");
    }
}

/// Runs in the forked child after installing the filter. Returns the process
/// exit code (0 = enforced as expected).
fn filter_probe() -> libc::c_int {
    // Same precondition as the real entry path (`restrict_self` sets it).
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return 10;
        }
    }
    if deny_child_processes().is_err() {
        return 11;
    }
    // Threads must keep working: this exercises the `clone3` -> `clone`
    // fallback and the `CLONE_THREAD` allowance.
    if std::thread::spawn(|| 40 + 2).join().unwrap_or(0) != 42 {
        return 12;
    }
    // Raw `fork` must be denied with `EPERM`.
    unsafe {
        if libc::fork() >= 0 {
            return 13;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
            return 14;
        }
    }
    // Spawning a helper process must fail, whether libc uses `posix_spawn`
    // (`clone` without `CLONE_THREAD`) or `fork`+`exec`.
    if std::process::Command::new("/bin/true").status().is_ok() {
        return 15;
    }
    0
}
