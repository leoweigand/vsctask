//! Turning a parsed task into something runnable.

use crate::model::{Arg, Options, Quoting, Task, merge_options};
use crate::vars::Context;
use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub label: String,
    #[serde(flatten)]
    pub exec: Exec,
    /// The command as a single shell line — what a pane or a `sh -c` wants.
    pub shell_line: String,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub is_background: bool,
    /// From `presentation.panel`; a frontend uses it to decide reuse.
    pub panel: Option<String>,
    /// From `presentation.group`; tasks sharing it belong side by side.
    pub group: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Exec {
    /// Handed to a shell, which does the word splitting.
    Shell { command: String, args: Vec<String> },
    /// Executed directly; no shell involved, so no quoting hazards.
    Process { program: String, args: Vec<String> },
}

/// A task that only declares `dependsOn` has nothing of its own to run.
pub fn is_composite(task: &Task) -> bool {
    task.command.is_none() && !task.depends_on.labels().is_empty()
}

pub fn resolve(task: &Task, file_options: Option<&Options>, ctx: &Context) -> Result<Resolved> {
    let command = task
        .command
        .as_deref()
        .with_context(|| format!("task {:?} has no command", task.label))?;

    let options = match &task.options {
        Some(o) => merge_options(file_options, o),
        None => file_options.cloned().unwrap_or_default(),
    };

    let command = ctx.resolve(command)?;
    let args: Vec<Arg> = task
        .args
        .iter()
        .map(|a| ctx.resolve(a.value()).map(|v| a.with_value(v)))
        .collect::<Result<_>>()?;

    let cwd = match &options.cwd {
        Some(c) => PathBuf::from(ctx.resolve(c)?),
        None => ctx.workspace_folder.clone(),
    };
    let env = options
        .env
        .iter()
        .map(|(k, v)| ctx.resolve(v).map(|v| (k.clone(), v)))
        .collect::<Result<BTreeMap<_, _>>>()?;

    let kind = task.kind.as_deref().unwrap_or("shell");
    let plain: Vec<String> = args.iter().map(|a| a.value().to_string()).collect();

    let (exec, shell_line) = match kind {
        // A shell task with no args goes through verbatim: the shell is meant
        // to see the pipes, globs and `&&` the author wrote.
        "shell" if args.is_empty() => (
            Exec::Shell { command: command.clone(), args: Vec::new() },
            command.clone(),
        ),
        "shell" => {
            let mut line = quote(&command, Quoting::Auto);
            for a in &args {
                line.push(' ');
                line.push_str(&quote(a.value(), a.quoting()));
            }
            (Exec::Shell { command: command.clone(), args: plain }, line)
        }
        "process" => {
            // Quoted only so the line is readable; execution skips the shell.
            let mut line = quote(&command, Quoting::Strong);
            for a in &plain {
                line.push(' ');
                line.push_str(&quote(a, Quoting::Strong));
            }
            (Exec::Process { program: command.clone(), args: plain }, line)
        }
        other => bail!(
            "task {:?} has type {other:?}; only \"shell\" and \"process\" are supported \
             (task providers such as npm or gulp are a VS Code extension feature)",
            task.label
        ),
    };

    let presentation = task.presentation.as_ref();
    Ok(Resolved {
        label: task.label.clone(),
        exec,
        shell_line,
        cwd,
        env,
        is_background: task.is_background,
        panel: presentation.and_then(|p| p.panel.clone()),
        group: presentation.and_then(|p| p.group.clone()),
    })
}

/// POSIX-shell quoting following VS Code's rules.
fn quote(value: &str, quoting: Quoting) -> String {
    match quoting {
        Quoting::Strong => single(value),
        Quoting::Weak => format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")),
        Quoting::Escape => value
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "_-./:=@+".contains(c) {
                    c.to_string()
                } else {
                    format!("\\{c}")
                }
            })
            .collect(),
        // VS Code quotes only when it has to: when the value has whitespace.
        Quoting::Auto => {
            if value.is_empty() || value.chars().any(char::is_whitespace) {
                single(value)
            } else {
                value.to_string()
            }
        }
    }
}

fn single(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}
