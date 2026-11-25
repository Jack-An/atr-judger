use std::ffi::{CString, c_char, c_int, c_uint, c_void};
use std::io::{self, Error};
use std::ptr::NonNull;

const ALLOW: u32 = 0x7fff_0000;
const ERRNO: u32 = 0x0005_0000;
const MASKED_EQ: c_int = 7;
const CLONE_NAMESPACE_FLAGS: u64 = 0x7e02_0080; // NEW{USER,PID,NET,NS,UTS,IPC,CGROUP,TIME}

#[repr(C)]
struct ArgComparison {
    arg: c_uint,
    op: c_int,
    mask: u64,
    value: u64,
}

#[link(name = "seccomp")]
unsafe extern "C" {
    fn seccomp_init(default_action: u32) -> *mut c_void;
    fn seccomp_release(ctx: *mut c_void);
    fn seccomp_load(ctx: *mut c_void) -> c_int;
    fn seccomp_syscall_resolve_name(name: *const c_char) -> c_int;
    fn seccomp_rule_add_array(
        ctx: *mut c_void,
        action: u32,
        syscall: c_int,
        count: c_uint,
        comparisons: *const ArgComparison,
    ) -> c_int;
}

// No network, namespace management, kernel instrumentation, or cross-process tracing.
// Some entries are only defined on one of the supported native architectures.
const SYSCALLS: &str = "
    access arch_prctl brk capget chdir chmod clock_getres clock_gettime clock_nanosleep
    close close_range creat dup dup2 dup3 epoll_create epoll_create1 epoll_ctl epoll_pwait
    epoll_pwait2 epoll_wait eventfd eventfd2 execve exit exit_group faccessat faccessat2
    fadvise64 fallocate fchdir fchmod fchmodat fcntl fdatasync flock fork fstat fstatfs
    fsync ftruncate futex futex_waitv getcpu getcwd getdents getdents64 getegid geteuid
    getgid getgroups getitimer getpeername getpgid getpgrp getpid getppid getpriority
    getrandom getresgid getresuid getrlimit getrusage getsid gettid gettimeofday getuid
    inotify_add_watch inotify_init inotify_init1 inotify_rm_watch ioctl kill link linkat
    lseek lstat madvise membarrier mincore mkdir mkdirat mmap mprotect mremap munmap
    nanosleep newfstatat open openat openat2 pause pipe pipe2 poll ppoll prctl pread64
    preadv preadv2 prlimit64 pselect6 pwrite64 pwritev pwritev2 read readahead readlink
    readlinkat readv rename renameat renameat2 restart_syscall rmdir rseq rt_sigaction
    rt_sigpending rt_sigprocmask rt_sigqueueinfo rt_sigreturn rt_sigsuspend rt_sigtimedwait
    rt_tgsigqueueinfo sched_getaffinity sched_getparam sched_getscheduler sched_yield select
    set_robust_list set_tid_address setitimer setpgid setrlimit setsid sigaltstack signalfd
    signalfd4 stat statfs statx symlink symlinkat sysinfo tgkill time timer_create timer_delete
    timer_gettime timer_settime timerfd_create timerfd_gettime timerfd_settime times tkill
    truncate umask uname unlink unlinkat utime utimensat utimes vfork wait4 waitid write writev
";

pub fn install() -> io::Result<()> {
    // SAFETY: libseccomp owns the context, and Filter releases it once.
    let ctx = unsafe { seccomp_init(ERRNO | libc::EPERM as u32) };
    let filter = Filter(NonNull::new(ctx).ok_or_else(|| Error::other("seccomp_init failed"))?);
    for name in SYSCALLS.split_whitespace() {
        filter.rule(name, ALLOW, &[])?;
    }
    // Force libc to use the argument-filtered clone syscall for thread creation.
    filter.rule("clone3", ERRNO | libc::ENOSYS as u32, &[])?;
    filter.rule(
        "clone",
        ALLOW,
        &[ArgComparison {
            arg: 0,
            op: MASKED_EQ,
            mask: CLONE_NAMESPACE_FLAGS,
            value: 0,
        }],
    )?;
    // SAFETY: The context is live and all preceding rules were validated.
    result(unsafe { seccomp_load(filter.0.as_ptr()) })
}

struct Filter(NonNull<c_void>);

impl Filter {
    fn rule(&self, name: &str, action: u32, comparison: &[ArgComparison]) -> io::Result<()> {
        let name = CString::new(name).map_err(Error::other)?;
        // SAFETY: name is a valid C string. Negative numbers are pseudo syscalls.
        let syscall = unsafe { seccomp_syscall_resolve_name(name.as_ptr()) };
        if syscall < 0 {
            return Ok(());
        }
        // SAFETY: The array lives for this call, and count is its length.
        result(unsafe {
            seccomp_rule_add_array(
                self.0.as_ptr(),
                action,
                syscall,
                comparison.len() as u32,
                comparison.as_ptr(),
            )
        })
    }
}

impl Drop for Filter {
    fn drop(&mut self) {
        // SAFETY: Filter owns this context and releases it once.
        unsafe { seccomp_release(self.0.as_ptr()) }
    }
}

fn result(code: c_int) -> io::Result<()> {
    if code < 0 {
        Err(Error::from_raw_os_error(-code))
    } else {
        Ok(())
    }
}
