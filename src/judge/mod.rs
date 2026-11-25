mod languages;
mod model;

pub use languages::Registry;
pub use model::{BatchOutcome, BatchRequest, Outcome, Request};

use crate::sandbox::{self, Program, Sandbox};
use languages::Recipe;
use model::{Case, Limits, Status};
use std::fs;
use std::io::{self, Error};
use std::path::Path;

const MIB: u64 = 1024 * 1024;

pub struct Engine {
    pub languages: Registry,
    sandbox: Sandbox,
}

impl Engine {
    pub fn new(languages: Registry, sandbox: Sandbox) -> Self {
        Self { languages, sandbox }
    }

    pub fn probe(&self) -> io::Result<()> {
        let mut startup = self.judge(Request {
            language: "python".to_string(),
            source_code: "print('ready')".into(),
            stdin: String::new(),
            expected_output: None,
            limits: Limits::default(),
        })?;
        if startup.status != Status::Accepted || startup.files.stdout != "ready\n" {
            return Err(Error::other(std::mem::take(&mut startup.files.stderr)));
        }
        Ok(())
    }

    pub fn judge(&self, request: Request) -> io::Result<Outcome> {
        let batch = self.judge_batch(BatchRequest {
            language: request.language,
            source_code: request.source_code,
            cases: vec![Case {
                stdin: request.stdin,
                expected_output: request.expected_output,
            }],
            limits: request.limits,
        })?;
        batch
            .compilation
            .or_else(|| batch.results.into_iter().next())
            .ok_or_else(|| Error::other("missing single case result"))
    }

    pub fn judge_batch(&self, request: BatchRequest) -> io::Result<BatchOutcome> {
        let workspace = tempfile::tempdir()?;
        let source = workspace.path().join("source");
        let artifact = workspace.path().join("artifact");
        fs::write(&source, request.source_code.as_bytes())?;
        let recipe = self
            .languages
            .get(&request.language)
            .ok_or_else(|| Error::other("unknown language"))?;

        let compiled = recipe.compile.is_some();
        if let Some(compilation) = self.compile(&source, &artifact, recipe, &request.limits)? {
            return Ok(BatchOutcome {
                compilation: Some(compilation),
                results: vec![],
            });
        }

        let limits = request.limits;
        let budget = sandbox::Limits {
            cpu_ms: limits.cpu_ms,
            wall_ms: limits.wall_ms,
            memory: (limits.memory_kb + recipe.memory_overhead_kb) * 1024,
            processes: recipe.processes,
            output: limits.output_bytes,
        };
        let program = Program {
            argv: recipe.run.clone(),
            input: recipe
                .compile
                .as_ref()
                .map_or(&recipe.source, |compile| &compile.artifact)
                .clone(),
            artifact: None,
            environment: recipe.environment.clone(),
        };
        let mut results = Vec::with_capacity(request.cases.len());
        for case in request.cases {
            let mut outcome = Outcome::from(self.sandbox.execute(
                if compiled { &artifact } else { &source },
                None,
                &program,
                &case.stdin,
                &budget,
            )?);
            if outcome.status == Status::Accepted
                && case
                    .expected_output
                    .is_some_and(|expected| expected.trim() != outcome.files.stdout.trim())
            {
                outcome.status = Status::WrongAnswer;
            }
            results.push(outcome);
        }
        Ok(BatchOutcome {
            compilation: None,
            results,
        })
    }

    fn compile(
        &self,
        source: &Path,
        artifact: &Path,
        recipe: &Recipe,
        limits: &Limits,
    ) -> io::Result<Option<Outcome>> {
        let Some(compile) = &recipe.compile else {
            return Ok(None);
        };
        let program = Program {
            argv: compile.argv.clone(),
            input: recipe.source.clone(),
            artifact: Some(compile.artifact.clone()),
            environment: recipe.environment.clone(),
        };
        let mut outcome = Outcome::from(self.sandbox.execute(
            source,
            Some(artifact),
            &program,
            "",
            &sandbox::Limits {
                cpu_ms: 15_000,
                wall_ms: 30_000,
                memory: 768 * MIB,
                processes: 96,
                output: limits.output_bytes,
            },
        )?);
        if outcome.status == Status::Accepted {
            Ok(None)
        } else {
            outcome.status = Status::Compilation;
            Ok(Some(outcome))
        }
    }
}
