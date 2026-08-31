//! The Herdr plugin: a popup task picker that places tasks in panes.
//!
//! Two entrypoints, matching Herdr's plugin model:
//!
//! * `pick` runs as a plugin *action*, so Herdr hands it the focused pane in
//!   `HERDR_PLUGIN_CONTEXT_JSON`. It captures that context into a handoff file
//!   and opens the popup, because a popup entrypoint gets no context of its own.
//! * `picker` runs inside the popup, filters the tasks, and places the chosen
//!   one.

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::plan;
use crate::resolve::Resolved;
use crate::vars;
use crate::workspace::Workspace;

/// The subset of Herdr's invocation context we use.
#[derive(Debug, Deserialize)]
struct Invocation {
    focused_pane_id: Option<String>,
    focused_pane_cwd: Option<String>,
    workspace_cwd: Option<String>,
}

/// Where a task should land. Written by `pick`, read by `picker` inside the
/// popup, and built from flags by `launch`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Target {
    pane_id: String,
    /// Where the pane was: the search root for tasks.json, and what
    /// `${workspaceFolder}` resolves against.
    pane_cwd: PathBuf,
}

/// Action entrypoint: capture context, then open the popup.
///
/// Herdr runs actions detached, so an error here would otherwise be silent.
/// Report it as a notification and let the plugin log keep the detail.
pub fn pick() -> Result<()> {
    match pick_inner() {
        Ok(()) => Ok(()),
        Err(err) => {
            notify("vsctask", &format!("{err:#}"));
            Err(err)
        }
    }
}

fn pick_inner() -> Result<()> {
    let raw = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .context("no HERDR_PLUGIN_CONTEXT_JSON; run this as a Herdr plugin action")?;
    let inv: Invocation =
        serde_json::from_str(&raw).context("cannot parse HERDR_PLUGIN_CONTEXT_JSON")?;

    let pane_id = inv.focused_pane_id.context("Herdr reported no focused pane")?;
    // The action's own cwd is the plugin root, so the focused pane's cwd is the
    // only sensible place to start looking for tasks.json.
    let pane_cwd = inv
        .focused_pane_cwd
        .or(inv.workspace_cwd)
        .map(PathBuf::from)
        .context("Herdr reported no cwd for the focused pane")?;

    // Discovery deliberately happens in the popup: a detached action has no
    // reliable way to show the user an error, and the popup always does.
    let handoff = Target { pane_id, pane_cwd };

    let path = state_dir()?.join("handoff.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&handoff)?)
        .with_context(|| format!("cannot write {}", path.display()))?;

    herdr(&[
        "plugin", "pane", "open",
        "--plugin", "vsctask",
        "--entrypoint", "picker",
        "--placement", "popup",
        "--width", "70%",
        "--height", "60%",
        "--env", &format!("VSCTASK_HANDOFF={}", path.display()),
        "--focus",
    ])?;
    Ok(())
}

/// Popup entrypoint: filter the tasks, then place the chosen one.
pub fn picker() -> Result<()> {
    let path = std::env::var("VSCTASK_HANDOFF")
        .context("no VSCTASK_HANDOFF; the popup was opened without a handoff")?;
    let handoff: Target = serde_json::from_slice(&std::fs::read(&path)?)
        .with_context(|| format!("cannot parse {path}"))?;

    let ws = Workspace::discover(&handoff.pane_cwd)?;
    let Some(label) = choose(&ws)? else {
        return Ok(()); // the user pressed escape
    };

    let ctx = context_for(&ws, &handoff);
    let plan = plan::build(&ws, &label, &ctx)?;
    place(&plan, &label, &handoff, &ws.tasks_path)
}

/// Report the environment a plugin entrypoint actually gets. Herdr spawns
/// plugin processes with a minimal PATH, so the picker's dependencies have to
/// be checked rather than assumed.
pub fn doctor() -> Result<()> {
    println!("PATH={}", std::env::var("PATH").unwrap_or_default());
    println!("cwd={}", std::env::current_dir()?.display());
    for tool in ["fzf", "herdr"] {
        match which(tool) {
            Some(p) => println!("{tool}: {}", p.display()),
            None => println!("{tool}: NOT FOUND"),
        }
    }
    Ok(())
}

/// Resolve a binary against PATH.
fn which(tool: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(tool))
            .find(|c| c.is_file())
    })
}

/// Place a named task without the popup. This is what a key bound straight to
/// a task uses, and it is the headless path for testing.
pub fn launch(ws: &Workspace, label: &str, pane_id: Option<&str>) -> Result<()> {
    let pane_id = match pane_id {
        Some(p) => p.to_string(),
        None => std::env::var("HERDR_PANE_ID")
            .context("no --pane given and no HERDR_PANE_ID in the environment")?,
    };
    let v = herdr(&["pane", "get", &pane_id])?;
    let pane_cwd = v["result"]["pane"]["cwd"]
        .as_str()
        .map(PathBuf::from)
        .with_context(|| format!("Herdr reported no cwd for pane {pane_id}"))?;

    let target = Target { pane_id, pane_cwd };
    let ctx = context_for(ws, &target);
    let plan = plan::build(ws, label, &ctx)?;
    place(&plan, label, &target, &ws.tasks_path)
}

/// Run fzf over the task list. fzf selects the first match, so Enter on a
/// filtered list runs exactly what the user is looking at.
fn choose(ws: &Workspace) -> Result<Option<String>> {
    let mut lines = String::new();
    for t in ws.visible_tasks() {
        let summary = if crate::resolve::is_composite(&t) {
            format!("→ {}", t.depends_on.labels().join(", "))
        } else {
            t.command.clone().unwrap_or_default()
        };
        lines.push_str(&format!("{}\t{}\n", t.label, summary));
    }
    if lines.is_empty() {
        bail!("{} declares no tasks", ws.tasks_path.display());
    }

    // Call ourselves by path: the popup's PATH is not ours to assume.
    let me = std::env::current_exe().context("cannot find my own path")?;
    let preview = format!(
        "{} --file {} plan {{1}} 2>&1",
        shell_quote(&me.display().to_string()),
        shell_quote(&ws.tasks_path.display().to_string())
    );
    let mut child = Command::new("fzf")
        .args([
            "--ansi",
            "--reverse",
            "--height=100%",
            "--delimiter=\t",
            "--with-nth=1,2",
            "--prompt=task> ",
            "--header=enter: run  ·  esc: cancel",
            "--preview-window=down,50%",
            "--preview",
            &preview,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("cannot start fzf; the picker needs fzf on PATH")?;

    use std::io::Write;
    child.stdin.take().unwrap().write_all(lines.as_bytes())?;
    let out = child.wait_with_output()?;
    // fzf: 0 chose something, 1 matched nothing, 130 was interrupted or escaped.
    // Anything else is a real failure and must not look like a cancellation.
    match out.status.code() {
        Some(0) => {}
        Some(1) | Some(130) => return Ok(None),
        other => bail!(
            "fzf exited with {}",
            other.map_or_else(|| "a signal".to_string(), |c| c.to_string())
        ),
    }
    let selected = String::from_utf8(out.stdout)?;
    let label = selected.trim_end_matches('\n').split('\t').next().unwrap_or_default();
    if label.is_empty() { Ok(None) } else { Ok(Some(label.to_string())) }
}

fn context_for(ws: &Workspace, handoff: &Target) -> vars::Context {
    let mut ctx = vars::Context::new(ws.root.clone(), handoff.pane_cwd.clone());
    ctx.config = ws.settings.clone();
    ctx
}

/// Place a plan's tasks into panes.
///
/// A single stage means everything can start at once, so each task gets its own
/// pane. Several stages need ordering that panes cannot express, so the whole
/// plan runs in one pane under `vsctask run`, which sequences it properly.
fn place(plan: &plan::Plan, label: &str, handoff: &Target, tasks_path: &Path) -> Result<()> {
    let single_stage: Vec<&Resolved> = match plan.stages.as_slice() {
        [only] => only.iter().collect(),
        _ => {
            let me = std::env::current_exe().context("cannot find my own path")?;
            let line = format!(
                "{} --file {} run {}",
                shell_quote(&me.display().to_string()),
                shell_quote(&tasks_path.display().to_string()),
                shell_quote(label)
            );
            let target = target_pane(&handoff.pane_id, &handoff.pane_cwd)?;
            rename(&target, label);
            return run_in(&target, &line);
        }
    };

    // The first task may reuse the focused pane; the rest always get their own.
    let mut anchor = handoff.pane_id.clone();
    for (i, task) in single_stage.iter().enumerate() {
        let pane = if i == 0 {
            target_pane(&handoff.pane_id, &task.cwd)?
        } else {
            split_from(&anchor, &task.cwd)?
        };
        anchor = pane.clone();
        rename(&pane, &task.label);
        run_in(&pane, &command_line(task))?;
    }
    Ok(())
}

/// The focused pane if it is free, otherwise a fresh split beside it.
fn target_pane(pane_id: &str, cwd: &Path) -> Result<String> {
    if is_available(pane_id)? {
        return Ok(pane_id.to_string());
    }
    split_from(pane_id, cwd)
}

/// A pane is free when the only thing in its foreground is its own shell.
fn is_available(pane_id: &str) -> Result<bool> {
    let v = herdr(&["pane", "process-info", "--pane", pane_id])?;
    let info = &v["result"]["process_info"];
    let shell_pid = info["shell_pid"].as_i64();
    let busy = info["foreground_processes"]
        .as_array()
        .map(|ps| ps.iter().any(|p| p["pid"].as_i64() != shell_pid))
        .unwrap_or(false);
    Ok(!busy)
}

/// Split wide panes to the right and tall ones down, so neither dimension
/// collapses. Terminal cells are about twice as tall as they are wide, so a
/// visually square pane has width ≈ 2 × height.
fn split_from(pane_id: &str, cwd: &Path) -> Result<String> {
    let direction = match pane_rect(pane_id)? {
        Some((w, h)) if w > h * 2 => "right",
        Some(_) => "down",
        None => "right",
    };
    let v = herdr(&[
        "pane", "split",
        "--pane", pane_id,
        "--direction", direction,
        "--cwd", &cwd.display().to_string(),
        "--no-focus",
    ])?;
    v["result"]["pane"]["pane_id"]
        .as_str()
        .map(str::to_string)
        .context("pane split returned no pane id")
}

fn pane_rect(pane_id: &str) -> Result<Option<(i64, i64)>> {
    let v = herdr(&["pane", "layout", "--pane", pane_id])?;
    let panes = v["result"]["layout"]["panes"].as_array().cloned().unwrap_or_default();
    for p in panes {
        if p["pane_id"].as_str() == Some(pane_id) {
            let r = &p["rect"];
            if let (Some(w), Some(h)) = (r["width"].as_i64(), r["height"].as_i64()) {
                return Ok(Some((w, h)));
            }
        }
    }
    Ok(None)
}

/// `cd` first so the task runs where its `options.cwd` says, then export the
/// task's env, then the command itself.
fn command_line(task: &Resolved) -> String {
    let mut line = format!("cd {}", shell_quote(&task.cwd.display().to_string()));
    for (k, v) in &task.env {
        line.push_str(&format!(" && export {k}={}", shell_quote(v)));
    }
    // A subshell keeps the `&&` guard covering the whole command: without it a
    // task like `echo up; sleep 600` would run its tail even if the cd failed.
    line.push_str(&format!(" && ( {} )", task.shell_line));
    line
}

fn run_in(pane_id: &str, line: &str) -> Result<()> {
    herdr(&["pane", "run", pane_id, line])?;
    Ok(())
}

/// A label on the pane makes a wall of servers readable. Not worth failing over.
fn rename(pane_id: &str, label: &str) {
    let _ = herdr(&["pane", "rename", pane_id, label]);
}

/// Best-effort user-visible message. Never worth failing over.
fn notify(title: &str, body: &str) {
    let _ = herdr(&["notification", "show", title, "--body", body, "--sound", "request"]);
}

fn herdr(args: &[&str]) -> Result<serde_json::Value> {
    let bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
    let out = Command::new(&bin)
        .args(args)
        .output()
        .with_context(|| format!("cannot run {bin}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        bail!("herdr {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    // Some commands succeed with no output at all.
    if stdout.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&stdout)
        .with_context(|| format!("herdr {} returned no JSON: {stdout}", args.join(" ")))
}

fn state_dir() -> Result<PathBuf> {
    let dir = std::env::var("HERDR_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("vsctask"));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;
    Ok(dir)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
