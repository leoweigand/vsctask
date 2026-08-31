//! VS Code variable substitution.
//!
//! Only `command`, `args` and `options` are substituted, matching VS Code.
//! Editor-scoped variables (`${file}` and friends) have no meaning outside the
//! editor, so they resolve only when the caller supplies a file with `--file`.

use anyhow::{Context as _, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Context {
    pub workspace_folder: PathBuf,
    pub cwd: PathBuf,
    /// Set by `--file`; unlocks the editor-scoped variables.
    pub file: Option<PathBuf>,
    /// Values for `${input:id}`, pre-resolved by the caller.
    pub inputs: BTreeMap<String, String>,
    /// Flattened `.vscode/settings.json`, for `${config:a.b.c}`.
    pub config: BTreeMap<String, String>,
}

impl Context {
    pub fn new(workspace_folder: PathBuf, cwd: PathBuf) -> Self {
        Self {
            workspace_folder,
            cwd,
            file: None,
            inputs: BTreeMap::new(),
            config: BTreeMap::new(),
        }
    }

    /// Expand every `${...}` in `s`. Unknown or unavailable variables are an
    /// error rather than a silent empty string, so a broken task fails loudly.
    pub fn resolve(&self, s: &str) -> Result<String> {
        let mut out = String::with_capacity(s.len());
        let bytes = s.as_bytes();
        let mut i = 0;

        while i < s.len() {
            if bytes[i] == b'$' && i + 1 < s.len() && bytes[i + 1] == b'{' {
                let Some(end) = s[i + 2..].find('}').map(|o| i + 2 + o) else {
                    // Unterminated: treat the rest as literal text.
                    out.push_str(&s[i..]);
                    break;
                };
                let name = &s[i + 2..end];
                out.push_str(&self.lookup(name)?);
                i = end + 1;
            } else {
                let ch = s[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }
        Ok(out)
    }

    fn lookup(&self, name: &str) -> Result<String> {
        if let Some(var) = name.strip_prefix("env:") {
            // VS Code substitutes an empty string for an unset variable.
            return Ok(std::env::var(var).unwrap_or_default());
        }
        if let Some(key) = name.strip_prefix("config:") {
            return self
                .config
                .get(key)
                .cloned()
                .with_context(|| format!("${{config:{key}}} is not set in .vscode/settings.json"));
        }
        if let Some(id) = name.strip_prefix("input:") {
            return self
                .inputs
                .get(id)
                .cloned()
                .with_context(|| format!("no value supplied for ${{input:{id}}}"));
        }
        if let Some(folder) = name.strip_prefix("workspaceFolder:") {
            // Multi-root: we only know the one folder we loaded.
            if self.basename() == folder {
                return Ok(self.workspace_folder.display().to_string());
            }
            bail!("${{workspaceFolder:{folder}}} refers to a workspace folder we did not load");
        }

        match name {
            "workspaceFolder" => Ok(self.workspace_folder.display().to_string()),
            "workspaceFolderBasename" => Ok(self.basename().to_string()),
            "cwd" => Ok(self.cwd.display().to_string()),
            "userHome" => Ok(home()?.display().to_string()),
            "pathSeparator" | "/" => Ok(std::path::MAIN_SEPARATOR.to_string()),
            "execPath" | "defaultBuildTask" => {
                bail!("${{{name}}} has no meaning outside VS Code")
            }
            "file" | "relativeFile" | "relativeFileDirname" | "fileBasename"
            | "fileBasenameNoExtension" | "fileDirname" | "fileDirnameBasename"
            | "fileExtname" => self.file_var(name),
            "lineNumber" | "selectedText" => {
                bail!("${{{name}}} needs an open editor; there is none here")
            }
            _ => bail!("unknown variable ${{{name}}}"),
        }
    }

    fn file_var(&self, name: &str) -> Result<String> {
        let file = self
            .file
            .as_deref()
            .with_context(|| format!("${{{name}}} needs a file; pass --file <path>"))?;
        let dir = file.parent().unwrap_or(Path::new(""));
        let stem_of = |p: &Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();

        Ok(match name {
            "file" => file.display().to_string(),
            "fileBasename" => stem_of(file),
            "fileBasenameNoExtension" => {
                file.file_stem().unwrap_or_default().to_string_lossy().into_owned()
            }
            "fileExtname" => file
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default(),
            "fileDirname" => dir.display().to_string(),
            "fileDirnameBasename" => stem_of(dir),
            "relativeFile" => relative(file, &self.workspace_folder),
            "relativeFileDirname" => relative(dir, &self.workspace_folder),
            _ => unreachable!(),
        })
    }

    fn basename(&self) -> &str {
        self.workspace_folder
            .file_name()
            .map(|s| s.to_str().unwrap_or_default())
            .unwrap_or_default()
    }
}

fn relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).display().to_string()
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("cannot determine the home directory")
}

/// Flatten nested JSON into dotted keys, the form `${config:a.b}` uses.
pub fn flatten_config(value: &serde_json::Value, prefix: &str, out: &mut BTreeMap<String, String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten_config(v, &key, out);
            }
        }
        serde_json::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        serde_json::Value::Null => {}
        other => {
            out.insert(prefix.to_string(), other.to_string());
        }
    }
}
