mod cgroup;
mod namespace;
mod seccomp;

use cgroup::CgroupRoot;
pub use namespace::stage_main;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Error, Read, Seek, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub(super) const MAX_ARTIFACT: u64 = 8 * 1024 * 1024;

pub struct Sandbox {
    cgroups: CgroupRoot,
    executable: PathBuf,
    runtime: PathBuf,
}

#[derive(Serialize, Deserialize)]
pub struct Program {
    pub argv: Vec<String>,
    pub input: String,
    pub artifact: Option<String>,
    pub environment: BTreeMap<String, String>,
}

pub struct Limits {
    pub cpu_ms: u64,
    pub wall_ms: u64,
    pub memory: u64,
    pub processes: u32,
    pub output: usize,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Exit {
    Success,
    Runtime,
    Time,
    Memory,
    Output,
}

pub struct Streams {
    pub stdout: String,
    pub stderr: String,
}

pub struct Execution {
    pub exit: Exit,
    pub cpu_ns: u64,
    pub memory_bytes: u64,
    pub streams: Streams,
}

impl Sandbox {
    pub fn new(runtime: PathBuf, cgroup_path: &Path) -> io::Result<Self> {
        Ok(Self {
            cgroups: CgroupRoot::initialize(cgroup_path)?,
            executable: std::env::current_exe()?,
            runtime,
        })
    }

    pub fn execute(
        &self,
        input: &Path,
        artifact: Option<&Path>,
        program: &Program,
        stdin: &str,
        budget: &Limits,
    ) -> io::Result<Execution> {
        let workspace = tempfile::tempdir()?;
        let root = workspace.path().join("root");
        let descriptor = workspace.path().join("program");
        fs::create_dir(&root)?;
        serde_json::to_writer(File::create(&descriptor)?, program)?;
        let group = self.cgroups.create(budget.memory, budget.processes)?;
        let mut data = tempfile::tempfile()?;
        data.write_all(stdin.as_bytes())?;
        data.rewind()?;
        data.set_permissions(fs::Permissions::from_mode(0o444))?;
        // Reopen stdin read-only before exposing it to the sandbox.
        let data = File::open(format!(
            "/proc/self/fd/{}",
            std::os::fd::AsRawFd::as_raw_fd(&data)
        ))?;
        let mut child = Command::new(&self.executable)
            .env_clear()
            .arg("--stage")
            .arg(group.path())
            .arg(&self.runtime)
            .arg(&root)
            .arg(input)
            .arg(artifact.unwrap_or_else(|| Path::new("-")))
            .arg(&descriptor)
            .arg(std::process::id().to_string())
            .stdin(data)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let exceeded = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_millis(budget.wall_ms);
        let (exit, stop, files) = thread::scope(|scope| -> io::Result<_> {
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| Error::other("missing stdout pipe"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| Error::other("missing stderr pipe"))?;
            let stdout = scope.spawn(|| capture(stdout, budget.output, &exceeded));
            let stderr = scope.spawn(|| capture(stderr, budget.output, &exceeded));
            let mut stop = None;
            let waited = (|| -> io::Result<_> {
                loop {
                    if let Some(exit) = child.try_wait()? {
                        return Ok(exit);
                    }
                    if exceeded.load(Ordering::Relaxed) {
                        stop = Some(Exit::Output);
                        break;
                    }
                    if Instant::now() >= deadline || group.cpu_micros()? >= budget.cpu_ms * 1000 {
                        stop = Some(Exit::Time);
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                group.kill()?;
                // The trusted helper may still be starting and outside the job cgroup.
                let _ = child.kill();
                child.wait()
            })();
            let killed = group.kill();
            if waited.is_err() || killed.is_err() {
                let _ = child.kill();
                let _ = child.wait();
            }
            let exit = waited?;
            killed?;
            let stdout = stdout
                .join()
                .map_err(|_| Error::other("stdout collector panicked"))??;
            let stderr = stderr
                .join()
                .map_err(|_| Error::other("stderr collector panicked"))??;
            Ok((exit, stop, Streams { stdout, stderr }))
        })?;

        let cpu_micros = group.cpu_micros()?;
        let status = if stop == Some(Exit::Output) || exceeded.load(Ordering::Relaxed) {
            Exit::Output
        } else if group.oom()? {
            Exit::Memory
        } else if stop == Some(Exit::Time) || cpu_micros >= budget.cpu_ms * 1000 {
            Exit::Time
        } else {
            match exit.code() {
                Some(0) => Exit::Success,
                Some(1) => Exit::Runtime,
                _ => return Err(Error::other(files.stderr)),
            }
        };
        Ok(Execution {
            exit: status,
            cpu_ns: cpu_micros * 1000,
            memory_bytes: group.memory_peak()?,
            streams: files,
        })
    }
}

fn capture(mut reader: impl Read, limit: usize, exceeded: &AtomicBool) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let size = reader.read(&mut chunk)?;
        if size == 0 {
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
        let take = size.min(limit - bytes.len());
        bytes.extend_from_slice(&chunk[..take]);
        if take < size {
            exceeded.store(true, Ordering::Relaxed);
        }
    }
}
