//! `vsctask` — read VS Code tasks.json, resolve tasks, run them or hand the
//! resolved plan to a frontend as JSON.

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use vsctask::model::Input;
use vsctask::resolve::{self, Exec};
use vsctask::vars::Context;
use vsctask::{plan, workspace::Workspace};

#[derive(Parser)]
#[command(version, about = "Run VS Code tasks.json tasks outside VS Code")]
struct Cli {
    /// Path to a tasks.json. Defaults to the nearest one at or above --dir.
    #[arg(long, global = true)]
    file: Option<PathBuf>,

    /// Where to start looking for .vscode/tasks.json.
    #[arg(long, global = true)]
    dir: Option<PathBuf>,

    /// Value for an ${input:id} variable. Repeatable: --input id=value
    #[arg(long = "input", global = true, value_name = "ID=VALUE")]
    inputs: Vec<String>,

    /// A file to resolve ${file} and friends against.
    #[arg(long = "for-file", global = true)]
    for_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the tasks in the file.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print one task's resolved command, cwd and env.
    Show {
        label: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the shell line for a task, ready to paste into a terminal.
    Emit { label: String },
    /// Print the full dependency plan as stages of concurrent tasks.
    Plan {
        label: String,
        #[arg(long)]
        json: bool,
    },
    /// Herdr plugin entrypoints. Not meant to be typed by hand.
    #[command(subcommand)]
    Herdr(HerdrCmd),
    /// Run a task and its dependencies.
    Run {
        label: String,
        /// Print what would run instead of running it.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum HerdrCmd {
    /// Plugin action: capture the focused pane and open the picker popup.
    Pick,
    /// Popup entrypoint: filter the tasks and place the chosen one.
    Picker,
    /// Report the environment a plugin entrypoint gets.
    Doctor,
    /// Place a named task in a pane, skipping the picker.
    Launch {
        label: String,
        /// Pane to start from. Defaults to $HERDR_PANE_ID.
        #[arg(long)]
        pane: Option<String>,
    },
}

fn main() {
    if let Err(err) = run() {
        eprintln!("vsctask: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    // These discover their workspace from Herdr's context, not from the cwd.
    match &cli.command {
        Cmd::Herdr(HerdrCmd::Pick) => return vsctask::herdr::pick(),
        Cmd::Herdr(HerdrCmd::Picker) => return vsctask::herdr::picker(),
        Cmd::Herdr(HerdrCmd::Doctor) => return vsctask::herdr::doctor(),
        _ => {}
    }

    let start = match &cli.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir()?,
    };

    let ws = match &cli.file {
        Some(f) => Workspace::load(f)?,
        None => Workspace::discover(&start)?,
    };

    let mut ctx = Context::new(ws.root.clone(), start);
    ctx.config = ws.settings.clone();
    ctx.file = cli.for_file.clone();
    ctx.inputs = parse_inputs(&cli.inputs, &ws.tasks.inputs)?;

    match &cli.command {
        Cmd::List { json } => list(&ws, *json),
        Cmd::Show { label, json } => show(&ws, &ctx, label, *json),
        Cmd::Emit { label } => emit(&ws, &ctx, label),
        Cmd::Plan { label, json } => show_plan(&ws, &ctx, label, *json),
        Cmd::Run { label, dry_run } => run_task(&ws, &ctx, label, *dry_run),
        Cmd::Herdr(HerdrCmd::Launch { label, pane }) => {
            vsctask::herdr::launch(&ws, label, pane.as_deref())
        }
        Cmd::Herdr(_) => unreachable!("handled above"),
    }
}

/// `--input id=value` pairs, falling back to each input's declared default.
fn parse_inputs(pairs: &[String], declared: &[Input]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for input in declared {
        if let Some(default) = &input.default {
            out.insert(input.id.clone(), default.clone());
        }
    }
    for pair in pairs {
        let (id, value) = pair
            .split_once('=')
            .with_context(|| format!("--input expects ID=VALUE, got {pair:?}"))?;
        out.insert(id.to_string(), value.to_string());
    }
    Ok(out)
}

fn list(ws: &Workspace, json: bool) -> Result<()> {
    let tasks = ws.visible_tasks();
    if json {
        let rows: Vec<_> = tasks
            .iter()
            .map(|t| {
                serde_json::json!({
                    "label": t.label,
                    "type": t.kind.clone().unwrap_or_else(|| "shell".into()),
                    "command": t.command,
                    "dependsOn": t.depends_on.labels(),
                    "composite": resolve::is_composite(t),
                    "detail": t.detail,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    let width = tasks.iter().map(|t| t.label.len()).max().unwrap_or(0);
    for t in &tasks {
        let summary = if resolve::is_composite(t) {
            format!("→ {}", t.depends_on.labels().join(", "))
        } else {
            t.command.clone().unwrap_or_default()
        };
        println!("{:width$}  {summary}", t.label, width = width);
    }
    Ok(())
}

fn show(ws: &Workspace, ctx: &Context, label: &str, json: bool) -> Result<()> {
    let task = ws.find(label)?;
    if resolve::is_composite(&task) {
        bail!("{label:?} only declares dependsOn; use `plan` to see what it runs");
    }
    let r = resolve::resolve(&task, ws.tasks.options.as_ref(), ctx)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        println!("label:   {}", r.label);
        println!("command: {}", r.shell_line);
        println!("cwd:     {}", r.cwd.display());
        for (k, v) in &r.env {
            println!("env:     {k}={v}");
        }
        if r.is_background {
            println!("background: true");
        }
    }
    Ok(())
}

fn emit(ws: &Workspace, ctx: &Context, label: &str) -> Result<()> {
    let task = ws.find(label)?;
    let r = resolve::resolve(&task, ws.tasks.options.as_ref(), ctx)?;
    println!("{}", r.shell_line);
    Ok(())
}

fn show_plan(ws: &Workspace, ctx: &Context, label: &str, json: bool) -> Result<()> {
    let plan = plan::build(ws, label, ctx)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }
    for (i, stage) in plan.stages.iter().enumerate() {
        println!("stage {}:", i + 1);
        for r in stage {
            println!("  {}  ({})  [{}]", r.label, r.shell_line, r.cwd.display());
        }
    }
    Ok(())
}

fn run_task(ws: &Workspace, ctx: &Context, label: &str, dry_run: bool) -> Result<()> {
    let plan = plan::build(ws, label, ctx)?;

    for stage in &plan.stages {
        if dry_run {
            for r in stage {
                println!("(cd {} && {})", r.cwd.display(), r.shell_line);
            }
            continue;
        }

        // Everything in a stage starts together, then we wait for the lot.
        let mut children = Vec::new();
        for r in stage {
            let mut cmd = match &r.exec {
                Exec::Shell { command, args } => {
                    let mut c = Command::new(shell());
                    c.arg("-c");
                    if args.is_empty() {
                        c.arg(command);
                    } else {
                        c.arg(&r.shell_line);
                    }
                    c
                }
                Exec::Process { program, args } => {
                    let mut c = Command::new(program);
                    c.args(args);
                    c
                }
            };
            cmd.current_dir(&r.cwd).envs(&r.env);
            let child = cmd
                .spawn()
                .with_context(|| format!("cannot start task {:?}", r.label))?;
            children.push((r.label.clone(), child));
        }

        for (label, mut child) in children {
            let status = child.wait()?;
            if !status.success() {
                bail!("task {label:?} exited with {status}");
            }
        }
    }
    Ok(())
}

fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}
