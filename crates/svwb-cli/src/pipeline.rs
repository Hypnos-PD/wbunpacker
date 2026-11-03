use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct Pipeline {
    pub version: u32,
    #[serde(default)]
    pub env_file: Option<String>,
    #[serde(default)]
    pub defaults: PipelineDefaults,
    #[serde(default)]
    pub steps: Vec<PipelineStep>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PipelineDefaults {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PipelineStep {
    pub id: String,
    #[serde(default)]
    pub description: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub optional_args: Vec<OptionalArgs>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OptionalArgs {
    pub when_env: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedStep {
    pub id: String,
    pub description: String,
    pub command: Vec<String>,
    pub cwd: String,
    pub depends_on: Vec<String>,
}

pub struct PipelineContext {
    variables: BTreeMap<String, String>,
}

impl Pipeline {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("read pipeline manifest: {}", path.display()))?;
        let pipeline: Self = toml::from_str(&raw)
            .with_context(|| format!("parse pipeline manifest: {}", path.display()))?;
        pipeline.validate()?;
        Ok(pipeline)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("pipeline version must be 1");
        }
        if self.steps.is_empty() {
            bail!("pipeline has no steps");
        }
        let known: HashSet<&str> = self.steps.iter().map(|step| step.id.as_str()).collect();
        if known.len() != self.steps.len() {
            bail!("pipeline step ids must be unique");
        }
        for step in &self.steps {
            if step.id.trim().is_empty() || step.command.is_empty() {
                bail!("pipeline step id and command must not be empty");
            }
            for dependency in &step.depends_on {
                if !known.contains(dependency.as_str()) {
                    bail!("step {} depends on unknown step {dependency}", step.id);
                }
            }
        }
        self.topological()?;
        Ok(())
    }

    pub fn topological(&self) -> Result<Vec<&PipelineStep>> {
        let by_id: HashMap<&str, &PipelineStep> = self
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step))
            .collect();
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut ordered = Vec::with_capacity(self.steps.len());

        fn visit<'a>(
            step: &'a PipelineStep,
            by_id: &HashMap<&str, &'a PipelineStep>,
            visiting: &mut HashSet<&'a str>,
            visited: &mut HashSet<&'a str>,
            ordered: &mut Vec<&'a PipelineStep>,
        ) -> Result<()> {
            if visited.contains(step.id.as_str()) {
                return Ok(());
            }
            if !visiting.insert(step.id.as_str()) {
                bail!("pipeline dependency cycle at step {}", step.id);
            }
            for dependency in &step.depends_on {
                let target = by_id
                    .get(dependency.as_str())
                    .copied()
                    .with_context(|| format!("unknown dependency {dependency}"))?;
                visit(target, by_id, visiting, visited, ordered)?;
            }
            visiting.remove(step.id.as_str());
            visited.insert(step.id.as_str());
            ordered.push(step);
            Ok(())
        }

        for step in &self.steps {
            visit(step, &by_id, &mut visiting, &mut visited, &mut ordered)?;
        }
        Ok(ordered)
    }

    pub fn select(
        &self,
        from: Option<&str>,
        only: &[String],
        skip: &[String],
    ) -> Result<Vec<&PipelineStep>> {
        if from.is_some() && !only.is_empty() {
            bail!("--from and --only cannot be used together");
        }
        let ordered = self.topological()?;
        let by_id: HashMap<&str, &PipelineStep> = ordered
            .iter()
            .map(|step| (step.id.as_str(), *step))
            .collect();

        let mut selected: HashSet<&str> = HashSet::new();
        if let Some(from) = from {
            let position = ordered
                .iter()
                .position(|step| step.id == from)
                .with_context(|| format!("unknown --from step {from}"))?;
            selected.extend(ordered[position..].iter().map(|step| step.id.as_str()));
        } else if !only.is_empty() {
            fn add_with_dependencies<'a>(
                id: &'a str,
                by_id: &HashMap<&str, &'a PipelineStep>,
                selected: &mut HashSet<&'a str>,
            ) -> Result<()> {
                let step = by_id
                    .get(id)
                    .copied()
                    .with_context(|| format!("unknown --only step {id}"))?;
                if !selected.insert(step.id.as_str()) {
                    return Ok(());
                }
                for dependency in &step.depends_on {
                    add_with_dependencies(dependency, by_id, selected)?;
                }
                Ok(())
            }
            for id in only {
                add_with_dependencies(id, &by_id, &mut selected)?;
            }
        } else {
            selected.extend(ordered.iter().map(|step| step.id.as_str()));
        }

        let skipped: HashSet<&str> = skip.iter().map(String::as_str).collect();
        for id in &skipped {
            if !by_id.contains_key(id) {
                bail!("unknown --skip step {id}");
            }
        }
        Ok(ordered
            .into_iter()
            .filter(|step| {
                selected.contains(step.id.as_str()) && !skipped.contains(step.id.as_str())
            })
            .collect())
    }

    pub fn context(&self, manifest: &Path) -> Result<PipelineContext> {
        let project_root = manifest
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .with_context(|| format!("resolve pipeline directory: {}", manifest.display()))?;
        let mut variables = std::env::vars().collect::<BTreeMap<_, _>>();
        variables.insert("PROJECT_ROOT".into(), project_root.display().to_string());
        if let Ok(executable) = std::env::current_exe() {
            variables.insert("SVWB_EXE".into(), executable.display().to_string());
        }
        if let Some(env_file) = &self.env_file {
            let path = project_root.join(env_file);
            if path.exists() {
                for (key, value) in parse_env_file(&path)? {
                    variables.entry(key).or_insert(value);
                }
            }
        }
        Ok(PipelineContext { variables })
    }

    pub fn plan(
        &self,
        context: &PipelineContext,
        steps: &[&PipelineStep],
    ) -> Result<Vec<PlannedStep>> {
        steps
            .iter()
            .map(|step| {
                let mut command = expand_all(&step.command, &context.variables)?;
                for optional in &step.optional_args {
                    if context
                        .variables
                        .get(&optional.when_env)
                        .is_some_and(|value| !value.is_empty())
                    {
                        command.extend(expand_all(&optional.args, &context.variables)?);
                    }
                }
                let cwd = expand(
                    step.cwd
                        .as_deref()
                        .or(self.defaults.cwd.as_deref())
                        .unwrap_or("${PROJECT_ROOT}"),
                    &context.variables,
                )?;
                Ok(PlannedStep {
                    id: step.id.clone(),
                    description: step.description.clone(),
                    command,
                    cwd,
                    depends_on: step.depends_on.clone(),
                })
            })
            .collect()
    }
}

impl PipelineContext {
    pub fn step_env(
        &self,
        pipeline: &Pipeline,
        step: &PipelineStep,
    ) -> Result<BTreeMap<String, String>> {
        let mut env = BTreeMap::new();
        if let Some(executable) = self.variables.get("WBU_EXE") {
            env.insert("WBU_EXE".into(), executable.clone());
        }
        for source in [&pipeline.defaults.env, &step.env] {
            for (key, value) in source {
                env.insert(key.clone(), expand(value, &self.variables)?);
            }
        }
        Ok(env)
    }

    pub fn run_step(&self, pipeline: &Pipeline, step: &PipelineStep, dry_run: bool) -> Result<()> {
        let planned = pipeline
            .plan(self, &[step])?
            .into_iter()
            .next()
            .context("planned step missing")?;
        let env = self.step_env(pipeline, step)?;
        if dry_run {
            println!(
                "[dry-run] {} cwd={} env={} {}",
                planned.id,
                planned.cwd,
                if env.is_empty() {
                    "-".into()
                } else {
                    env.keys().cloned().collect::<Vec<_>>().join(",")
                },
                shell_join(&planned.command)
            );
            return Ok(());
        }

        println!(">>> {} {}", planned.id, planned.description);
        let status = Command::new(&planned.command[0])
            .args(&planned.command[1..])
            .current_dir(&planned.cwd)
            .envs(&env)
            .status()
            .with_context(|| format!("start pipeline step {}", planned.id))?;
        if !status.success() {
            bail!("pipeline step {} exited with {status}", planned.id);
        }
        Ok(())
    }
}

pub fn expand_all(values: &[String], variables: &BTreeMap<String, String>) -> Result<Vec<String>> {
    values
        .iter()
        .map(|value| expand(value, variables))
        .collect()
}

pub fn expand(value: &str, variables: &BTreeMap<String, String>) -> Result<String> {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .with_context(|| format!("unterminated variable in {value}"))?;
        let name = &after[..end];
        let replacement = variables
            .get(name)
            .with_context(|| format!("pipeline variable {name} is not set"))?;
        output.push_str(replacement);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn parse_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for (index, raw) in std::fs::read_to_string(path)
        .with_context(|| format!("read environment file: {}", path.display()))?
        .lines()
        .enumerate()
    {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            bail!(
                "{}:{}: invalid environment entry",
                path.display(),
                index + 1
            );
        };
        let mut value = value.trim();
        if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            value = &value[1..value.len() - 1];
        }
        values.insert(key.trim().into(), value.into());
    }
    Ok(values)
}

fn shell_join(command: &[String]) -> String {
    command
        .iter()
        .map(|argument| {
            if argument
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_./:=@".contains(ch))
            {
                argument.clone()
            } else {
                format!("{argument:?}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
version = 1

[defaults]
cwd = "${PROJECT_ROOT}"

[[steps]]
id = "extract"
description = "extract"
command = ["wbu", "master"]

[[steps]]
id = "derive"
description = "derive"
command = ["node", "derive.mjs"]
depends_on = ["extract"]

[[steps]]
id = "publish"
description = "publish"
command = ["svwb", "catalog", "publish"]
depends_on = ["derive"]
optional_args = [{when_env = "DRY_RUN", args = ["--dry-run"]}]
"#;

    fn pipeline() -> Pipeline {
        toml::from_str(MANIFEST).expect("pipeline")
    }

    #[test]
    fn validates_and_orders_dependencies() {
        let pipeline = pipeline();
        pipeline.validate().unwrap();
        let ordered = pipeline.topological().unwrap();
        assert_eq!(
            ordered
                .iter()
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>(),
            ["extract", "derive", "publish"]
        );
    }

    #[test]
    fn detects_cycles_and_unknown_dependencies() {
        let mut invalid = pipeline();
        invalid.steps[0].depends_on.push("publish".into());
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );

        let mut invalid = pipeline();
        invalid.steps[1].depends_on.push("missing".into());
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .to_string()
                .contains("unknown step")
        );
    }

    #[test]
    fn selects_from_and_expands_optional_arguments() {
        let pipeline = pipeline();
        let selected = pipeline.select(Some("derive"), &[], &[]).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>(),
            ["derive", "publish"]
        );

        let mut variables = BTreeMap::new();
        variables.insert("PROJECT_ROOT".into(), "/repo".into());
        variables.insert("DRY_RUN".into(), "1".into());
        let context = PipelineContext { variables };
        let plan = pipeline.plan(&context, &selected).unwrap();
        assert_eq!(plan[0].cwd, "/repo");
        assert_eq!(plan[1].command.last().unwrap(), "--dry-run");
    }

    #[test]
    fn passes_wbu_executable_to_steps() {
        let pipeline = pipeline();
        let variables = BTreeMap::from([
            ("PROJECT_ROOT".into(), "/repo".into()),
            ("WBU_EXE".into(), "/tools/wbu".into()),
            ("MODE".into(), "test".into()),
        ]);
        let context = PipelineContext { variables };
        let env = context.step_env(&pipeline, &pipeline.steps[0]).unwrap();
        assert_eq!(env.get("WBU_EXE").map(String::as_str), Some("/tools/wbu"));
        assert!(!env.contains_key("MODE"));
    }

    #[test]
    fn only_includes_dependencies_and_skip_removes_steps() {
        let pipeline = pipeline();
        let selected = pipeline
            .select(None, &["publish".into()], &["extract".into()])
            .unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>(),
            ["derive", "publish"]
        );
    }
}
