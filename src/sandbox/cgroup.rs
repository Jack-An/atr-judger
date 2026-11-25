use std::fs::{self, File};
use std::io::{self, Error};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

static NEXT_GROUP: AtomicU64 = AtomicU64::new(0);

pub struct CgroupRoot(PathBuf);

impl CgroupRoot {
    pub fn initialize(path: &Path) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let domain = File::open(&path)?;
        let mut stat = MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: domain is open and stat is writable. The kernel initializes it on success.
        let filesystem = unsafe {
            if libc::fstatfs(domain.as_raw_fd(), stat.as_mut_ptr()) == -1 {
                return Err(Error::last_os_error());
            }
            stat.assume_init().f_type
        };
        if filesystem != libc::CGROUP2_SUPER_MAGIC as libc::c_long {
            return Err(Error::other("cgroup root is not on a cgroup v2 filesystem"));
        }
        if fs::read_to_string(path.join("cgroup.type"))?.trim() != "domain" {
            return Err(Error::other("a dedicated cgroup v2 domain is required"));
        }
        let controllers = fs::read_to_string(path.join("cgroup.controllers"))?;
        if ["cpu", "memory", "pids"]
            .iter()
            .any(|name| !controllers.split_whitespace().any(|item| item == *name))
        {
            return Err(Error::other(
                "cpu, memory and pids controllers must be delegated",
            ));
        }

        let manager = path.join("manager");
        match fs::create_dir(&manager) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        // A non-root domain must have an empty cgroup.procs to enable controllers.
        fs::write(manager.join("cgroup.procs"), std::process::id().to_string())?;
        fs::write(path.join("cgroup.subtree_control"), "+cpu +memory +pids")?;

        let root = Self(path);
        let probe = root.create(64 * 1024 * 1024, 8)?;
        probe.kill()?;
        probe.cpu_micros()?;
        probe.memory_peak()?;
        probe.oom()?;
        Ok(root)
    }

    pub fn create(&self, memory_bytes: u64, processes: u32) -> io::Result<Cgroup> {
        let id = NEXT_GROUP.fetch_add(1, Ordering::Relaxed);
        let path = self.0.join(format!("job-{}-{id}", std::process::id()));
        fs::create_dir(&path)?;
        let group = Cgroup(path);
        fs::write(group.0.join("memory.max"), memory_bytes.to_string())?;
        fs::write(group.0.join("memory.swap.max"), "0")?;
        fs::write(group.0.join("memory.oom.group"), "1")?;
        fs::write(group.0.join("pids.max"), processes.to_string())?;
        fs::write(group.0.join("cpu.max"), "100000 100000")?;
        Ok(group)
    }
}

pub struct Cgroup(PathBuf);

impl Cgroup {
    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn kill(&self) -> io::Result<()> {
        fs::write(self.0.join("cgroup.kill"), "1")
    }

    pub fn cpu_micros(&self) -> io::Result<u64> {
        field(&fs::read_to_string(self.0.join("cpu.stat"))?, "usage_usec")
    }

    pub fn memory_peak(&self) -> io::Result<u64> {
        parse(fs::read_to_string(self.0.join("memory.peak"))?.trim())
    }

    pub fn oom(&self) -> io::Result<bool> {
        let events = fs::read_to_string(self.0.join("memory.events"))?;
        Ok(field(&events, "oom_kill")? > 0 || field(&events, "oom")? > 0)
    }
}

impl Drop for Cgroup {
    fn drop(&mut self) {
        let _ = self.kill();
        for _ in 0..10 {
            if fs::remove_dir(&self.0).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        eprintln!("Could not remove job cgroup");
    }
}

fn field(contents: &str, name: &str) -> io::Result<u64> {
    let value = contents
        .lines()
        .filter_map(|line| line.split_once(' '))
        .find_map(|(key, value)| (key == name).then_some(value))
        .ok_or_else(|| Error::other(format!("missing cgroup counter {name}")))?;
    parse(value)
}

fn parse(value: &str) -> io::Result<u64> {
    value.parse().map_err(Error::other)
}
