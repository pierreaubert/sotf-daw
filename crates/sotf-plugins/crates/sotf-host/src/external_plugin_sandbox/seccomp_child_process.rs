//! Linux seccomp-bpf backend denying child-process creation for isolated
//! external plugin workers.
//!
//! Landlock confines what a worker can touch, but it cannot stop the worker
//! from spawning new processes. This filter blocks `fork`, `vfork`, `clone3`,
//! and `clone` without `CLONE_THREAD`, so the worker (and any plugin code it
//! loads) can still create threads but cannot fork helpers, shells, or
//! re-exec chains outside the supervisor's view.
//!
//! `clone3` is denied with `ENOSYS` rather than `EPERM` so libc thread
//! creation transparently falls back to `clone`, which the filter then allows
//! when `CLONE_THREAD` is set. Everything else is allowed: this is a
//! process-spawn gate, not a full syscall sandbox.

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
mod ffi {
    // Offsets into `struct seccomp_data` (stable kernel ABI).
    pub const NR_OFFSET: u32 = 0;
    pub const ARGS_OFFSET: u32 = 16;
    // `CLONE_THREAD` from <sched.h>; identical on all Linux architectures.
    pub const CLONE_THREAD: u32 = 0x0001_0000;
}

/// Install the child-process denial filter on the calling thread (and, with
/// `TSYNC`, every other thread of the process). Threads and processes created
/// afterwards inherit the filter.
///
/// Requires `no_new_privs`, which the Landlock entry path already sets before
/// this runs.
#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(super) fn deny_child_processes() -> Result<(), String> {
    let program = build_filter();
    let prog = libc::sock_fprog {
        len: program.len() as libc::c_ushort,
        filter: program.as_ptr() as *mut libc::sock_filter,
    };
    // `program` must outlive the syscall; it is a local and is not moved.
    let installed = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_TSYNC,
            &prog,
        )
    };
    if installed == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EINVAL) {
        // Kernel predates `TSYNC` support. The worker installs the sandbox
        // before spawning its threads, so a calling-thread-only filter still
        // covers everything created afterwards.
        let installed = unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                libc::SECCOMP_SET_MODE_FILTER,
                0,
                &prog,
            )
        };
        if installed == 0 {
            return Ok(());
        }
        return Err(format!(
            "failed to install child-process seccomp filter without TSYNC: {}",
            std::io::Error::last_os_error()
        ));
    }
    Err(format!(
        "failed to install child-process seccomp filter: {err}"
    ))
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
fn build_filter() -> Vec<libc::sock_filter> {
    use ffi::{ARGS_OFFSET, CLONE_THREAD, NR_OFFSET};

    const ALLOW: u32 = libc::SECCOMP_RET_ALLOW;
    const DENY_PROCESS: u32 = libc::SECCOMP_RET_ERRNO | (libc::EPERM as u32);
    // `ENOSYS` makes libc thread creation fall back from `clone3` to `clone`.
    const DENY_CLONE3: u32 = libc::SECCOMP_RET_ERRNO | (libc::ENOSYS as u32);

    let mut builder = Builder::new();
    let allow = builder.label();
    let deny = builder.label();
    let deny_clone3 = builder.label();
    let check_clone = builder.label();
    let check_fork = builder.label();
    let check_vfork = builder.label();
    let check_clone3 = builder.label();

    builder.emit_stmt(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, NR_OFFSET);
    // aarch64 has no `fork`/`vfork` syscalls; only `clone`/`clone3` exist.
    #[cfg(target_arch = "x86_64")]
    builder.emit_jump(
        libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
        libc::SYS_clone as u32,
        check_clone,
        check_fork,
    );
    #[cfg(not(target_arch = "x86_64"))]
    builder.emit_jump(
        libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
        libc::SYS_clone as u32,
        check_clone,
        check_clone3,
    );
    builder.bind(check_fork);
    #[cfg(target_arch = "x86_64")]
    builder.emit_jump(
        libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
        libc::SYS_fork as u32,
        deny,
        check_vfork,
    );
    builder.bind(check_vfork);
    #[cfg(target_arch = "x86_64")]
    builder.emit_jump(
        libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
        libc::SYS_vfork as u32,
        deny,
        check_clone3,
    );
    builder.bind(check_clone3);
    builder.emit_jump(
        libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
        libc::SYS_clone3 as u32,
        deny_clone3,
        allow,
    );
    builder.bind(check_clone);
    builder.emit_stmt(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, ARGS_OFFSET);
    builder.emit_stmt(libc::BPF_ALU | libc::BPF_AND | libc::BPF_K, CLONE_THREAD);
    builder.emit_jump(
        libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
        0,
        deny,
        allow,
    );
    builder.bind(deny);
    builder.emit_stmt(libc::BPF_RET | libc::BPF_K, DENY_PROCESS);
    builder.bind(deny_clone3);
    builder.emit_stmt(libc::BPF_RET | libc::BPF_K, DENY_CLONE3);
    builder.bind(allow);
    builder.emit_stmt(libc::BPF_RET | libc::BPF_K, ALLOW);
    builder.build()
}

/// Minimal classic-BPF assembler with labels so jump offsets stay correct.
#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
struct Builder {
    insns: Vec<libc::sock_filter>,
    targets: Vec<usize>,
    fixups: Vec<(usize, bool, usize)>,
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
impl Builder {
    fn new() -> Self {
        Self {
            insns: Vec::new(),
            targets: Vec::new(),
            fixups: Vec::new(),
        }
    }

    fn label(&mut self) -> usize {
        let id = self.targets.len();
        self.targets.push(usize::MAX);
        id
    }

    fn bind(&mut self, label: usize) {
        self.targets[label] = self.insns.len();
    }

    fn emit_stmt(&mut self, code: u32, k: u32) {
        self.insns.push(libc::sock_filter {
            code: code as u16,
            jt: 0,
            jf: 0,
            k,
        });
    }

    fn emit_jump(&mut self, code: u32, k: u32, jt: usize, jf: usize) {
        let index = self.insns.len();
        self.insns.push(libc::sock_filter {
            code: code as u16,
            jt: 0,
            jf: 0,
            k,
        });
        self.fixups.push((index, true, jt));
        self.fixups.push((index, false, jf));
    }

    fn build(mut self) -> Vec<libc::sock_filter> {
        for (index, is_jt, label) in std::mem::take(&mut self.fixups) {
            let target = self.targets[label];
            assert!(target != usize::MAX, "unbound seccomp label");
            assert!(target > index, "seccomp jumps only forward");
            let offset = (target - index - 1) as u8;
            if is_jt {
                self.insns[index].jt = offset;
            } else {
                self.insns[index].jf = offset;
            }
        }
        self.insns
    }
}

/// Fallback for Linux architectures without a hand-checked syscall table:
///
/// report the backend as unavailable so policy stays honest.
#[cfg(not(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64"))))]
pub(super) fn deny_child_processes() -> Result<(), String> {
    Err("child-process seccomp filter is not implemented for this architecture".to_string())
}

#[cfg(all(test, target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
mod tests {
    use super::*;

    #[test]
    fn filter_program_is_well_formed() {
        let program = build_filter();
        assert!(!(program.is_empty() || program.len() > 64));
        // First instruction loads the syscall number.
        assert_eq!(
            program[0].code as u32,
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS
        );
        assert_eq!(program[0].k, ffi::NR_OFFSET);
        // Default verdict is allow; every jump must land inside the program.
        let last = program.last().unwrap();
        assert_eq!(last.code as u32, libc::BPF_RET | libc::BPF_K);
        assert_eq!(last.k, libc::SECCOMP_RET_ALLOW);
        for (index, insn) in program.iter().enumerate() {
            let class = (insn.code as u32) & 0x07;
            if class == libc::BPF_JMP {
                assert!(index + 1 + insn.jt as usize <= program.len());
                assert!(index + 1 + insn.jf as usize <= program.len());
            }
        }
        // The filter must mention every process-spawn entry point.
        let watched: Vec<u32> = program
            .iter()
            .filter(|insn| (insn.code as u32) == (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K))
            .map(|insn| insn.k)
            .collect();
        assert!(watched.contains(&(libc::SYS_clone as u32)));
        assert!(watched.contains(&(libc::SYS_clone3 as u32)));
        #[cfg(target_arch = "x86_64")]
        {
            assert!(watched.contains(&(libc::SYS_fork as u32)));
            assert!(watched.contains(&(libc::SYS_vfork as u32)));
        }
    }
}
