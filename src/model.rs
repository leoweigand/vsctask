//! Serde model for the subset of `tasks.json` we care about.

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksFile {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub inputs: Vec<Input>,
    /// Defaults inherited by every task in the file.
    #[serde(default)]
    pub options: Option<Options>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// VS Code allows `taskName` as a legacy alias for `label`.
    #[serde(alias = "taskName")]
    pub label: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<Arg>,
    #[serde(default)]
    pub options: Option<Options>,
    #[serde(default)]
    pub depends_on: DependsOn,
    #[serde(default)]
    pub depends_order: DependsOrder,
    #[serde(default)]
    pub is_background: bool,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub group: Option<serde_json::Value>,
    /// Hidden from VS Code's Run Task picker; we hide it from `list` too.
    #[serde(default)]
    pub hide: bool,
    #[serde(default)]
    pub presentation: Option<Presentation>,
    #[serde(default)]
    pub run_options: Option<RunOptions>,

    /// Platform overrides. Only the host platform's key is applied.
    #[serde(default)]
    pub osx: Option<Box<PlatformOverride>>,
    #[serde(default)]
    pub linux: Option<Box<PlatformOverride>>,
    #[serde(default)]
    pub windows: Option<Box<PlatformOverride>>,
}

/// The fields a `osx`/`linux`/`windows` block may replace.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformOverride {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<Arg>>,
    #[serde(default)]
    pub options: Option<Options>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub shell: Option<ShellConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellConfig {
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

/// `dependsOn` is either a single label or a list of them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
pub enum DependsOn {
    One(String),
    Many(Vec<String>),
    #[serde(skip)]
    #[default]
    None,
}

impl DependsOn {
    pub fn labels(&self) -> Vec<&str> {
        match self {
            DependsOn::One(s) => vec![s.as_str()],
            DependsOn::Many(v) => v.iter().map(String::as_str).collect(),
            DependsOn::None => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependsOrder {
    /// VS Code's default: dependencies start together.
    #[default]
    Parallel,
    Sequence,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub id: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
}

impl Task {
    /// Fold the host platform's override block into the task.
    pub fn for_host(&self) -> Task {
        let over = if cfg!(target_os = "macos") {
            self.osx.as_deref()
        } else if cfg!(target_os = "windows") {
            self.windows.as_deref()
        } else {
            self.linux.as_deref()
        };
        let Some(over) = over else { return self.clone() };

        let mut t = self.clone();
        if let Some(c) = &over.command {
            t.command = Some(c.clone());
        }
        if let Some(a) = &over.args {
            t.args = a.clone();
        }
        if let Some(o) = &over.options {
            t.options = Some(merge_options(t.options.as_ref(), o));
        }
        t
    }
}

/// Later values win, but a missing value never clobbers a present one.
pub fn merge_options(base: Option<&Options>, over: &Options) -> Options {
    let mut out = base.cloned().unwrap_or_default();
    if over.cwd.is_some() {
        out.cwd = over.cwd.clone();
    }
    if over.shell.is_some() {
        out.shell = over.shell.clone();
    }
    out.env.extend(over.env.clone());
    out
}

/// An `args` entry: either a bare string or `{ value, quoting }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Arg {
    Plain(String),
    Quoted {
        value: String,
        #[serde(default)]
        quoting: Quoting,
    },
}

impl Arg {
    pub fn value(&self) -> &str {
        match self {
            Arg::Plain(s) => s,
            Arg::Quoted { value, .. } => value,
        }
    }

    pub fn quoting(&self) -> Quoting {
        match self {
            Arg::Plain(_) => Quoting::Auto,
            Arg::Quoted { quoting, .. } => *quoting,
        }
    }

    pub fn with_value(&self, v: String) -> Arg {
        match self {
            Arg::Plain(_) => Arg::Plain(v),
            Arg::Quoted { quoting, .. } => Arg::Quoted { value: v, quoting: *quoting },
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quoting {
    /// VS Code's default: quote only when the value contains whitespace.
    #[default]
    Auto,
    /// Single quotes; the shell evaluates nothing inside.
    Strong,
    /// Double quotes; the shell still expands variables.
    Weak,
    /// No quotes, special characters escaped individually.
    Escape,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presentation {
    #[serde(default)]
    pub reveal: Option<String>,
    #[serde(default)]
    pub echo: Option<bool>,
    #[serde(default)]
    pub focus: Option<bool>,
    /// `shared` | `dedicated` | `new`
    #[serde(default)]
    pub panel: Option<String>,
    /// Tasks sharing a group share a split terminal in VS Code.
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub clear: Option<bool>,
    #[serde(default)]
    pub close: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    /// `default` | `folderOpen`
    #[serde(default)]
    pub run_on: Option<String>,
    #[serde(default)]
    pub instance_limit: Option<u32>,
    #[serde(default)]
    pub reevaluate_on_rerun: Option<bool>,
}
