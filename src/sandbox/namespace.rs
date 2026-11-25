use super::{MAX_ARTIFACT, Program, seccomp};
use std::ffi::{CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Error, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

const SANDBOX_ID: u32 = 65534;

pub fn stage_main(args: impl Iterator<Item = OsString>) -> i32 {
    match stage(args.collect()) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(error) => {
            eprintln!("Stage setup failed: {error}");
            2
        }
    }
}

fn stage(args: Vec<OsString>) -> io::Result<bool> {
    let [cgroup, runtime, root, input, artifact, descriptor, parent] = args.as_slice() else {
        return Err(Error::other("invalid stage arguments"));
    };
    let parent = parent
        .to_str()
        .and_then(|pid| pid.parse::<libc::pid_t>().ok())
        .ok_or_else(|| Error::other("invalid supervisor PID"))?;
    terminate_with_parent()?;
    // SAFETY: getppid takes no arguments. Catch service exit before prctl ran.
    if unsafe { libc::getppid() } != parent {
        return Err(Error::other(
            "judger supervisor exited before initialization",
        ));
    }
    let program: Program = serde_json::from_reader(File::open(descriptor)?)?;
    fs::write(
        Path::new(cgroup).join("cgroup.procs"),
        std::process::id().to_string(),
    )?;
    // Prevent a sandbox process from tracing its trusted supervisor.
    // SAFETY: Only constants and integer arguments are passed to prctl and unshare.
    unsafe {
        checked(libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0))?;
        checked(libc::unshare(
            libc::CLONE_NEWNS
                | libc::CLONE_NEWPID
                | libc::CLONE_NEWNET
                | libc::CLONE_NEWIPC
                | libc::CLONE_NEWUTS,
        ))?;
    }

    // SAFETY: pidfd_open gets a handle to the current trusted helper.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0) };
    if fd == -1 {
        return Err(Error::last_os_error());
    }
    // SAFETY: The successful syscall returned a newly owned descriptor.
    let helper = unsafe { File::from_raw_fd(fd as libc::c_int) };

    // SAFETY: --stage starts before the HTTP runtime and has no other threads.
    // The first child becomes PID 1 of the new PID namespace.
    let pid = checked(unsafe { libc::fork() })?;
    if pid == 0 {
        let result = guard_namespace_parent(helper).and_then(|()| {
            run_in_namespace(
                Path::new(runtime),
                Path::new(root),
                Path::new(input),
                (artifact != "-").then(|| Path::new(artifact)),
                &program,
            )
        });
        let code = match result {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(error) => {
                eprintln!("Namespace setup failed: {error}");
                2
            }
        };
        // SAFETY: This exits only the namespace child without running parent teardown.
        unsafe { libc::_exit(code) }
    }

    drop(helper);
    let mut status = 0;
    loop {
        // SAFETY: pid is the child returned by fork and status is writable.
        match checked(unsafe { libc::waitpid(pid, &mut status, 0) }) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => {
                result?;
                break;
            }
        }
    }
    match libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status)) {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::other("namespace supervisor failed")),
    }
}

fn terminate_with_parent() -> io::Result<()> {
    // SAFETY: The arguments request delivery of SIGKILL to the caller on parent exit.
    checked(unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) }).map(|_| ())
}

fn guard_namespace_parent(helper: File) -> io::Result<()> {
    terminate_with_parent()?;
    // The parent's PID is outside this namespace. Check its inherited pidfd for
    // exit before PR_SET_PDEATHSIG took effect.
    let mut descriptor = libc::pollfd {
        fd: helper.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll reads/writes exactly one initialized descriptor.
    if checked(unsafe { libc::poll(&mut descriptor, 1, 0) })? != 0 {
        return Err(Error::other(
            "stage helper exited before namespace initialization",
        ));
    }
    Ok(())
}

fn run_in_namespace(
    runtime: &Path,
    root: &Path,
    input: &Path,
    artifact: Option<&Path>,
    program: &Program,
) -> io::Result<bool> {
    let mut input = File::open(input)?;
    let mut export = artifact
        .map(|path| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
        })
        .transpose()?;
    prepare_root(runtime, root)?;
    let work = Path::new("/work");
    let file = work.join(&program.input);
    io::copy(&mut input, &mut File::create(&file)?)?;
    fs::set_permissions(&file, fs::Permissions::from_mode(0o555))?;
    drop(input);
    let (executable, args) = program
        .argv
        .split_first()
        .ok_or_else(|| Error::other("missing program"))?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/usr/local/bin")
        .env("HOME", "/tmp")
        .env("LANG", "C.UTF-8")
        .env("TMPDIR", "/tmp")
        .envs(&program.environment)
        .current_dir(work);
    // SAFETY: This trusted PID 1 process is single-threaded. The hook runs in its
    // exec child after stdio is connected and before the submission starts.
    unsafe { command.pre_exec(drop_privileges) };
    let success = command.status()?.success();
    reap_descendants()?;

    if success && let (Some(export), Some(artifact)) = (export.as_mut(), &program.artifact) {
        let produced = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(work.join(artifact))?;
        let metadata = produced.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_ARTIFACT {
            return Ok(false);
        }
        let copied = io::copy(&mut produced.take(MAX_ARTIFACT + 1), export)?;
        if copied > MAX_ARTIFACT {
            return Ok(false);
        }
    }
    Ok(success)
}

fn prepare_root(runtime: &Path, root: &Path) -> io::Result<()> {
    let secure = libc::MS_NOSUID | libc::MS_NODEV;
    mount(
        None,
        Path::new("/"),
        None,
        libc::MS_REC | libc::MS_PRIVATE,
        None,
    )?;
    mount(
        Some(c"tmpfs"),
        root,
        Some(c"tmpfs"),
        secure,
        Some(c"size=64m,nr_inodes=4096,mode=0755"),
    )?;
    for directory in ["usr", "dev", "proc", "etc", "work", "tmp", ".old"] {
        fs::create_dir(root.join(directory))?;
    }
    bind_read_only(&runtime.join("usr"), &root.join("usr"))?;
    let java_config = runtime.join("etc/java-21-openjdk");
    if java_config.is_dir() {
        let java_target = root.join("etc/java-21-openjdk");
        fs::create_dir(&java_target)?;
        bind_read_only(&java_config, &java_target)?;
    }
    for directory in ["bin", "sbin", "lib", "lib64"] {
        std::os::unix::fs::symlink(format!("usr/{directory}"), root.join(directory))?;
    }
    fs::write(
        root.join("etc/passwd"),
        "nobody:x:65534:65534:Runner:/tmp:/usr/sbin/nologin\n",
    )?;
    fs::write(root.join("etc/group"), "nogroup:x:65534:\n")?;
    for directory in ["work", "tmp"] {
        let dir = root.join(directory);
        // SAFETY: dir is a fixed child of the private tmpfs.
        checked(unsafe { libc::chown(c_path(&dir)?.as_ptr(), SANDBOX_ID, SANDBOX_ID) })?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }

    // A separate device mount exposes only the three explicitly created devices.
    mount(
        Some(c"tmpfs"),
        &root.join("dev"),
        Some(c"tmpfs"),
        libc::MS_NOSUID | libc::MS_NOEXEC,
        Some(c"size=64k,nr_inodes=32,mode=0755"),
    )?;
    for (name, minor) in [("null", 3), ("zero", 5), ("urandom", 9)] {
        let path = c_path(&root.join("dev").join(name))?;
        // SAFETY: path is NUL-terminated and points into the private /dev mount.
        checked(unsafe {
            libc::mknod(
                path.as_ptr(),
                libc::S_IFCHR | 0o666,
                libc::makedev(1, minor),
            )
        })?;
        fs::set_permissions(
            root.join("dev").join(name),
            fs::Permissions::from_mode(0o666),
        )?;
    }
    std::os::unix::fs::symlink("/proc/self/fd", root.join("dev/fd"))?;
    for (descriptor, name) in ["stdin", "stdout", "stderr"].iter().enumerate() {
        std::os::unix::fs::symlink(
            format!("/proc/self/fd/{descriptor}"),
            root.join("dev").join(name),
        )?;
    }
    mount(
        Some(c"proc"),
        &root.join("proc"),
        Some(c"proc"),
        secure | libc::MS_RDONLY | libc::MS_NOEXEC,
        Some(c"hidepid=2,subset=pid"),
    )?;

    std::env::set_current_dir(root)?;
    // SAFETY: The new root is the private mount at cwd and .old is its empty child.
    let result = unsafe { libc::syscall(libc::SYS_pivot_root, c".".as_ptr(), c".old".as_ptr()) };
    if result == -1 {
        return Err(Error::last_os_error());
    }
    std::env::set_current_dir("/")?;
    // SAFETY: The detached mount is the old root of this namespace only.
    checked(unsafe { libc::umount2(c"/.old".as_ptr(), libc::MNT_DETACH) })?;
    fs::remove_dir("/.old")?;
    Ok(())
}

fn bind_read_only(source: &Path, target: &Path) -> io::Result<()> {
    let source = c_path(source)?;
    // A non-recursive bind excludes any submounts from the runtime.
    mount(Some(&source), target, None, libc::MS_BIND, None)?;
    mount(
        None,
        target,
        None,
        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
        None,
    )
}

fn drop_privileges() -> io::Result<()> {
    set_limit(libc::RLIMIT_CORE, 0)?;
    set_limit(libc::RLIMIT_FSIZE, MAX_ARTIFACT)?;
    set_limit(libc::RLIMIT_NOFILE, 128)?;
    // SAFETY: All pointers are null or kernel-independent scalar inputs. The child
    // has private process credentials and any failure prevents it from executing.
    unsafe {
        for capability in 0..=63 {
            let result = libc::prctl(libc::PR_CAPBSET_DROP, capability, 0, 0, 0);
            if result == -1 && Error::last_os_error().raw_os_error() != Some(libc::EINVAL) {
                return Err(Error::last_os_error());
            }
        }
        checked(libc::setgroups(0, std::ptr::null()))?;
        checked(libc::setresgid(SANDBOX_ID, SANDBOX_ID, SANDBOX_ID))?;
        checked(libc::setresuid(SANDBOX_ID, SANDBOX_ID, SANDBOX_ID))?;
        checked(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0))?;
        // Mark inherited descriptors close-on-exec without closing Rust's exec-error pipe.
        if libc::syscall(
            libc::SYS_close_range,
            3u32,
            u32::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        ) == -1
        {
            return Err(Error::last_os_error());
        }
    }
    seccomp::install()
}

fn set_limit(resource: libc::__rlimit_resource_t, limit: u64) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    // SAFETY: limit points to an initialized rlimit for the current process.
    checked(unsafe { libc::setrlimit(resource, &limit) }).map(|_| ())
}

fn reap_descendants() -> io::Result<()> {
    // SAFETY: PID 1 can address only members of its own PID namespace.
    if unsafe { libc::kill(-1, libc::SIGKILL) } == -1
        && Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    {
        return Err(Error::last_os_error());
    }
    loop {
        // SAFETY: waitpid does not write a status when given a null pointer.
        if unsafe { libc::waitpid(-1, std::ptr::null_mut(), 0) } == -1 {
            let error = Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ECHILD) => return Ok(()),
                Some(libc::EINTR) => continue,
                _ => return Err(error),
            }
        }
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(Error::other)
}

fn mount(
    source: Option<&std::ffi::CStr>,
    target: &Path,
    filesystem: Option<&std::ffi::CStr>,
    flags: libc::c_ulong,
    options: Option<&std::ffi::CStr>,
) -> io::Result<()> {
    fn ptr(value: Option<&std::ffi::CStr>) -> *const libc::c_char {
        value.map_or(std::ptr::null(), std::ffi::CStr::as_ptr)
    }
    let target = c_path(target)?;
    // SAFETY: Each provided pointer refers to a live C string for this call.
    checked(unsafe {
        libc::mount(
            ptr(source),
            target.as_ptr(),
            ptr(filesystem),
            flags,
            ptr(options).cast(),
        )
    })
    .map(|_| ())
}

fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result == -1 {
        Err(Error::last_os_error())
    } else {
        Ok(result)
    }
}
