use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Error, Read};
use std::path::{Component, Path};

const BUILTINS: &str = include_str!("languages.json");
const MAX_CONFIG_BYTES: u64 = 128 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recipe {
    pub source: String,
    pub compile: Option<Compile>,
    pub run: Vec<String>,
    pub processes: u32,
    #[serde(default)]
    pub memory_overhead_kb: u64,
    #[serde(default)]
    libraries: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Compile {
    pub argv: Vec<String>,
    pub artifact: String,
}

pub struct Registry(BTreeMap<String, Recipe>);

#[derive(Serialize)]
pub struct Entry<'a> {
    id: &'a str,
    compiled: bool,
    libraries: &'a [String],
}

impl Registry {
    pub fn load(runtime: &Path, extensions: Option<&Path>) -> io::Result<Self> {
        let mut recipes: BTreeMap<String, Recipe> = serde_json::from_str(BUILTINS)?;
        if let Some(extensions) = extensions {
            let mut document = Vec::new();
            File::open(extensions)?
                .take(MAX_CONFIG_BYTES + 1)
                .read_to_end(&mut document)?;
            if document.len() as u64 > MAX_CONFIG_BYTES {
                return Err(Error::other("language manifest exceeds 128 KiB"));
            }
            let additional: BTreeMap<String, Recipe> = serde_json::from_slice(&document)?;
            recipes.extend(additional);
        }
        for (id, recipe) in &recipes {
            if !check_id(id) || !recipe.valid(runtime) {
                return Err(Error::other(format!(
                    "invalid or unavailable language recipe: {id}"
                )));
            }
        }
        Ok(Self(recipes))
    }

    pub fn get(&self, id: &str) -> Option<&Recipe> {
        self.0.get(id)
    }

    pub fn list(&self) -> Vec<Entry<'_>> {
        self.0
            .iter()
            .map(|(id, recipe)| Entry {
                id,
                compiled: recipe.compile.is_some(),
                libraries: &recipe.libraries,
            })
            .collect()
    }
}

impl Recipe {
    fn valid(&self, runtime: &Path) -> bool {
        valid_filename(&self.source)
            && (4..=128).contains(&self.processes)
            && self.memory_overhead_kb <= 262_144
            && self.libraries.len() <= 128
            && self
                .libraries
                .iter()
                .all(|library| !library.is_empty() && library.len() <= 128)
            && self.environment.len() <= 16
            && self.environment.iter().all(|(name, value)| {
                (1..=64).contains(&name.len())
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    && value.len() <= 4096
                    && !value.contains('\0')
            })
            && self.compile.as_ref().is_none_or(|compile| {
                valid_filename(&compile.artifact) && valid_command(runtime, &compile.argv, None)
            })
            && valid_command(
                runtime,
                &self.run,
                self.compile.as_ref().map(|c| c.artifact.as_str()),
            )
    }
}

fn valid_command(runtime: &Path, argv: &[String], artifact: Option<&str>) -> bool {
    let Some(program) = argv.first() else {
        return false;
    };
    if argv.len() > 64
        || argv
            .iter()
            .any(|arg| arg.len() > 4096 || arg.contains('\0'))
    {
        return false;
    }
    if artifact.is_some_and(|name| *program == format!("/work/{name}")) {
        return true;
    }
    let Some(relative) = program.strip_prefix("/usr/") else {
        return false;
    };
    !relative.is_empty()
        && Path::new(relative)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && runtime.join("usr").join(relative).is_file()
}

fn valid_filename(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn check_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
