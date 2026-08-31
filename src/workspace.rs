//! Finding and loading a workspace's `.vscode` configuration.

use crate::model::TasksFile;
use crate::vars;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Workspace {
    /// The folder holding `.vscode`, i.e. `${workspaceFolder}`.
    pub root: PathBuf,
    pub tasks_path: PathBuf,
    pub tasks: TasksFile,
    pub settings: BTreeMap<String, String>,
}

impl Workspace {
    /// Load the nearest `.vscode/tasks.json` at or above `from`.
    pub fn discover(from: &Path) -> Result<Workspace> {
        let start = from
            .canonicalize()
            .with_context(|| format!("cannot resolve {}", from.display()))?;
        for dir in start.ancestors() {
            let candidate = dir.join(".vscode").join("tasks.json");
            if candidate.is_file() {
                return Workspace::load(&candidate);
            }
        }
        bail!("no .vscode/tasks.json found in {} or any parent", start.display())
    }

    pub fn load(tasks_path: &Path) -> Result<Workspace> {
        let root = tasks_path
            .parent()
            .and_then(Path::parent)
            .context("tasks.json must live in a .vscode directory")?
            .to_path_buf();

        let tasks: TasksFile = parse_jsonc(tasks_path)?;

        let settings_path = root.join(".vscode").join("settings.json");
        let mut settings = BTreeMap::new();
        if settings_path.is_file() {
            let value: serde_json::Value = parse_jsonc(&settings_path)?;
            vars::flatten_config(&value, "", &mut settings);
        }

        Ok(Workspace { root, tasks_path: tasks_path.to_path_buf(), tasks, settings })
    }

    /// Tasks with a platform override folded in, hidden ones dropped.
    pub fn visible_tasks(&self) -> Vec<crate::model::Task> {
        self.tasks.tasks.iter().filter(|t| !t.hide).map(|t| t.for_host()).collect()
    }

    pub fn find(&self, label: &str) -> Result<crate::model::Task> {
        self.tasks
            .tasks
            .iter()
            .find(|t| t.label == label)
            .map(|t| t.for_host())
            .with_context(|| {
                let known: Vec<&str> =
                    self.tasks.tasks.iter().map(|t| t.label.as_str()).collect();
                format!("no task labelled {label:?}. Known: {}", known.join(", "))
            })
    }
}

/// tasks.json is JSONC: comments and trailing commas are legal.
fn parse_jsonc<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    jsonc_parser::parse_to_serde_value(&text, &Default::default())
        .with_context(|| format!("cannot parse {}", path.display()))
}
