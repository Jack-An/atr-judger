use super::Registry;
use crate::sandbox::{Execution, Exit};
use serde::{Deserialize, Serialize};

const MAX_SOURCE: usize = 256 * 1024;
const MAX_INPUT: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub language: String,
    pub source_code: String,
    #[serde(default)]
    pub stdin: String,
    pub expected_output: Option<String>,
    #[serde(default)]
    pub limits: Limits,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub cpu_ms: u64,
    pub wall_ms: u64,
    pub memory_kb: u64,
    pub output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cpu_ms: 2_000,
            wall_ms: 5_000,
            memory_kb: 262_144,
            output_bytes: 65_536,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchRequest {
    pub language: String,
    pub source_code: String,
    pub cases: Vec<Case>,
    #[serde(default)]
    pub limits: Limits,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Case {
    #[serde(default)]
    pub stdin: String,
    pub expected_output: Option<String>,
}

impl Request {
    pub fn valid(&self, registry: &Registry) -> bool {
        registry.get(&self.language).is_some()
            && self.source_code.len() <= MAX_SOURCE
            && self.stdin.len() <= MAX_INPUT
            && valid_expected_output(self.expected_output.as_deref(), self.limits.output_bytes)
            && self.limits.valid()
    }
}

impl BatchRequest {
    pub fn valid(&self, registry: &Registry) -> bool {
        registry.get(&self.language).is_some()
            && self.source_code.len() <= MAX_SOURCE
            && (1..=16).contains(&self.cases.len())
            && self.limits.valid()
            && self
                .cases
                .iter()
                .map(|case| case.stdin.len())
                .sum::<usize>()
                <= MAX_INPUT
            && self.cases.iter().all(|case| {
                valid_expected_output(case.expected_output.as_deref(), self.limits.output_bytes)
            })
            && self.cases.len() as u64 * self.limits.wall_ms <= 120_000
            && self.cases.len() * self.limits.output_bytes <= MAX_INPUT
    }
}

fn valid_expected_output(value: Option<&str>, limit: usize) -> bool {
    value.is_none_or(|value| value.len() <= limit)
}

impl Limits {
    fn valid(&self) -> bool {
        (1..=65_000).contains(&self.cpu_ms)
            && (1..=70_000).contains(&self.wall_ms)
            && (2_048..=1_048_576).contains(&self.memory_kb)
            && (1..=262_144).contains(&self.output_bytes)
    }
}

#[derive(Serialize)]
pub struct BatchOutcome {
    pub compilation: Option<Outcome>,
    pub results: Vec<Outcome>,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
pub enum Status {
    #[serde(rename = "Accepted")]
    Accepted,
    #[serde(rename = "Wrong Answer")]
    WrongAnswer,
    #[serde(rename = "Compilation Error")]
    Compilation,
    #[serde(rename = "Time Limit Exceeded")]
    Time,
    #[serde(rename = "Memory Limit Exceeded")]
    Memory,
    #[serde(rename = "Output Limit Exceeded")]
    Output,
    #[serde(rename = "Nonzero Exit Status")]
    Runtime,
}

#[derive(Serialize)]
pub struct Outcome {
    pub status: Status,
    pub time: u64,
    pub memory: u64,
    pub files: Output,
}

#[derive(Serialize)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

impl From<Execution> for Outcome {
    fn from(execution: Execution) -> Self {
        let status = match execution.exit {
            Exit::Success => Status::Accepted,
            Exit::Runtime => Status::Runtime,
            Exit::Time => Status::Time,
            Exit::Memory => Status::Memory,
            Exit::Output => Status::Output,
        };
        Self {
            status,
            time: execution.cpu_ns,
            memory: execution.memory_bytes,
            files: Output {
                stdout: execution.streams.stdout,
                stderr: execution.streams.stderr,
            },
        }
    }
}
