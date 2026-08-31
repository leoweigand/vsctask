use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use vsctask::resolve::{self, Exec};
use vsctask::vars::Context;
use vsctask::{plan, workspace::Workspace};

/// Write a tasks.json into a fresh temp workspace and load it.
fn workspace(name: &str, body: &str) -> (PathBuf, Workspace) {
    let root = std::env::temp_dir().join(format!("vsctask-test-{name}"));
    let dotdir = root.join(".vscode");
    std::fs::create_dir_all(&dotdir).unwrap();
    std::fs::write(dotdir.join("tasks.json"), body).unwrap();
    let ws = Workspace::load(&dotdir.join("tasks.json")).unwrap();
    (root, ws)
}

fn ctx(root: &Path) -> Context {
    Context::new(root.canonicalize().unwrap(), root.canonicalize().unwrap())
}

fn line(ws: &Workspace, ctx: &Context, label: &str) -> String {
    let task = ws.find(label).unwrap();
    resolve::resolve(&task, ws.tasks.options.as_ref(), ctx).unwrap().shell_line
}

#[test]
fn parses_jsonc_with_comments_and_trailing_commas() {
    let (_root, ws) = workspace(
        "jsonc",
        r#"{
            // a comment
            "version": "2.0.0",
            "tasks": [
                { "label": "a", "command": "echo hi", }, /* another */
            ],
        }"#,
    );
    assert_eq!(ws.tasks.tasks.len(), 1);
    assert_eq!(ws.tasks.version.as_deref(), Some("2.0.0"));
}

#[test]
fn shell_task_without_args_passes_through_verbatim() {
    // The shell must still see the && and the glob the author wrote.
    let (root, ws) = workspace(
        "verbatim",
        r#"{"tasks":[{"label":"a","type":"shell","command":"cd x && ls *.rs | wc -l"}]}"#,
    );
    assert_eq!(line(&ws, &ctx(&root), "a"), "cd x && ls *.rs | wc -l");
}

#[test]
fn shell_task_with_args_quotes_only_what_needs_it() {
    let (root, ws) = workspace(
        "quoting",
        r#"{"tasks":[{
            "label":"a","type":"shell","command":"my prog",
            "args":[
                "plain",
                "has space",
                {"value":"$HOME/x","quoting":"strong"},
                {"value":"$HOME/y","quoting":"weak"},
                {"value":"a;b","quoting":"escape"}
            ]
        }]}"#,
    );
    assert_eq!(
        line(&ws, &ctx(&root), "a"),
        r#"'my prog' plain 'has space' '$HOME/x' "$HOME/y" a\;b"#
    );
}

#[test]
fn single_quotes_inside_a_value_are_escaped() {
    let (root, ws) = workspace(
        "apostrophe",
        r#"{"tasks":[{"label":"a","type":"shell","command":"echo","args":[{"value":"it's","quoting":"strong"}]}]}"#,
    );
    assert_eq!(line(&ws, &ctx(&root), "a"), r#"echo 'it'\''s'"#);
}

#[test]
fn process_tasks_skip_the_shell() {
    let (root, ws) = workspace(
        "process",
        r#"{"tasks":[{"label":"a","type":"process","command":"/bin/ls","args":["-l","my dir"]}]}"#,
    );
    let task = ws.find("a").unwrap();
    let r = resolve::resolve(&task, None, &ctx(&root)).unwrap();
    match r.exec {
        Exec::Process { program, args } => {
            assert_eq!(program, "/bin/ls");
            // Args reach the program untouched; no quoting is involved.
            assert_eq!(args, vec!["-l", "my dir"]);
        }
        _ => panic!("expected a process task"),
    }
}

#[test]
fn resolves_variables_in_command_cwd_and_env() {
    let (root, ws) = workspace(
        "vars",
        r#"{"tasks":[{
            "label":"a","type":"shell","command":"run ${workspaceFolderBasename}",
            "options":{"cwd":"${workspaceFolder}/sub","env":{"P":"${env:VSCTASK_TEST_VAR}"}}
        }]}"#,
    );
    unsafe { std::env::set_var("VSCTASK_TEST_VAR", "from-env") };
    let c = ctx(&root);
    let task = ws.find("a").unwrap();
    let r = resolve::resolve(&task, None, &c).unwrap();
    assert_eq!(r.shell_line, "run vsctask-test-vars");
    assert_eq!(r.cwd, c.workspace_folder.join("sub"));
    assert_eq!(r.env.get("P").unwrap(), "from-env");
}

#[test]
fn unknown_variables_fail_loudly() {
    let (root, ws) = workspace(
        "unknown",
        r#"{"tasks":[{"label":"a","command":"echo ${nonsense}"}]}"#,
    );
    let task = ws.find("a").unwrap();
    let err = resolve::resolve(&task, None, &ctx(&root)).unwrap_err();
    assert!(err.to_string().contains("unknown variable"), "{err}");
}

#[test]
fn editor_variables_explain_themselves_when_no_file_is_given() {
    let (root, ws) = workspace("editor", r#"{"tasks":[{"label":"a","command":"cat ${file}"}]}"#);
    let task = ws.find("a").unwrap();
    let err = resolve::resolve(&task, None, &ctx(&root)).unwrap_err();
    assert!(err.to_string().contains("--file"), "{err}");
}

#[test]
fn inputs_come_from_the_caller() {
    let (root, ws) = workspace(
        "inputs",
        r#"{
            "inputs":[{"id":"target","type":"pickString","options":["dev","prod"]}],
            "tasks":[{"label":"a","command":"deploy ${input:target}"}]
        }"#,
    );
    let mut c = ctx(&root);
    c.inputs = BTreeMap::from([("target".into(), "prod".into())]);
    assert_eq!(line(&ws, &c, "a"), "deploy prod");
}

#[test]
fn config_variables_read_vscode_settings() {
    let (root, _) = workspace(
        "config",
        r#"{"tasks":[{"label":"a","command":"use ${config:my.tool.path}"}]}"#,
    );
    std::fs::write(
        root.join(".vscode").join("settings.json"),
        r#"{ "my.tool": { "path": "/opt/tool" } }"#,
    )
    .unwrap();
    let ws = Workspace::load(&root.join(".vscode").join("tasks.json")).unwrap();
    let mut c = ctx(&root);
    c.config = ws.settings.clone();
    assert_eq!(line(&ws, &c, "a"), "use /opt/tool");
}

#[test]
fn platform_override_replaces_command_and_merges_options() {
    let (root, ws) = workspace(
        "platform",
        r#"{"tasks":[{
            "label":"a","command":"generic","options":{"env":{"KEEP":"1"}},
            "osx":{"command":"mac-only","options":{"env":{"EXTRA":"2"}}},
            "linux":{"command":"linux-only"},
            "windows":{"command":"win-only"}
        }]}"#,
    );
    let task = ws.find("a").unwrap();
    let r = resolve::resolve(&task, None, &ctx(&root)).unwrap();
    let expected = if cfg!(target_os = "macos") {
        "mac-only"
    } else if cfg!(target_os = "windows") {
        "win-only"
    } else {
        "linux-only"
    };
    assert_eq!(r.shell_line, expected);
    if cfg!(target_os = "macos") {
        // A merge, not a replacement: the base env survives.
        assert_eq!(r.env.get("KEEP").map(String::as_str), Some("1"));
        assert_eq!(r.env.get("EXTRA").map(String::as_str), Some("2"));
    }
}

#[test]
fn file_level_options_are_the_default_for_every_task() {
    let (root, ws) = workspace(
        "file-options",
        r#"{
            "options":{"cwd":"${workspaceFolder}/shared","env":{"BASE":"1"}},
            "tasks":[
                {"label":"inherits","command":"a"},
                {"label":"overrides","command":"b","options":{"cwd":"${workspaceFolder}/own"}}
            ]
        }"#,
    );
    let c = ctx(&root);
    let opts = ws.tasks.options.as_ref();

    let a = resolve::resolve(&ws.find("inherits").unwrap(), opts, &c).unwrap();
    assert_eq!(a.cwd, c.workspace_folder.join("shared"));
    assert_eq!(a.env.get("BASE").map(String::as_str), Some("1"));

    let b = resolve::resolve(&ws.find("overrides").unwrap(), opts, &c).unwrap();
    assert_eq!(b.cwd, c.workspace_folder.join("own"));
    // Overriding cwd must not drop the inherited env.
    assert_eq!(b.env.get("BASE").map(String::as_str), Some("1"));
}

#[test]
fn parallel_dependencies_share_one_stage() {
    let (root, ws) = workspace(
        "parallel",
        r#"{"tasks":[
            {"label":"all","dependsOn":["x","y","z"],"dependsOrder":"parallel"},
            {"label":"x","command":"x"},{"label":"y","command":"y"},{"label":"z","command":"z"}
        ]}"#,
    );
    let plan = plan::build(&ws, "all", &ctx(&root)).unwrap();
    // "all" itself runs nothing, so only the one stage of leaves remains.
    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].len(), 3);
}

#[test]
fn parallel_is_the_default_order() {
    let (root, ws) = workspace(
        "default-order",
        r#"{"tasks":[
            {"label":"all","dependsOn":["x","y"]},
            {"label":"x","command":"x"},{"label":"y","command":"y"}
        ]}"#,
    );
    let plan = plan::build(&ws, "all", &ctx(&root)).unwrap();
    assert_eq!(plan.stages.len(), 1);
}

#[test]
fn sequence_dependencies_get_one_stage_each_then_the_task_itself() {
    let (root, ws) = workspace(
        "sequence",
        r#"{"tasks":[
            {"label":"release","command":"publish","dependsOn":["clean","build","test"],"dependsOrder":"sequence"},
            {"label":"clean","command":"c"},{"label":"build","command":"b"},{"label":"test","command":"t"}
        ]}"#,
    );
    let plan = plan::build(&ws, "release", &ctx(&root)).unwrap();
    let labels: Vec<Vec<&str>> = plan
        .stages
        .iter()
        .map(|s| s.iter().map(|r| r.label.as_str()).collect())
        .collect();
    assert_eq!(labels, vec![vec!["clean"], vec!["build"], vec!["test"], vec!["release"]]);
}

#[test]
fn a_single_string_dependson_is_accepted() {
    let (root, ws) = workspace(
        "single-dep",
        r#"{"tasks":[
            {"label":"b","command":"b","dependsOn":"a"},
            {"label":"a","command":"a"}
        ]}"#,
    );
    let plan = plan::build(&ws, "b", &ctx(&root)).unwrap();
    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0][0].label, "a");
}

#[test]
fn a_shared_dependency_runs_once_before_both_users() {
    let (root, ws) = workspace(
        "diamond",
        r#"{"tasks":[
            {"label":"top","dependsOn":["left","right"]},
            {"label":"left","command":"l","dependsOn":"base"},
            {"label":"right","command":"r","dependsOn":"base"},
            {"label":"base","command":"b"}
        ]}"#,
    );
    let plan = plan::build(&ws, "top", &ctx(&root)).unwrap();
    let labels: Vec<Vec<&str>> = plan
        .stages
        .iter()
        .map(|s| s.iter().map(|r| r.label.as_str()).collect())
        .collect();
    assert_eq!(labels, vec![vec!["base"], vec!["left", "right"]]);
}

#[test]
fn a_dependency_cycle_is_reported() {
    let (root, ws) = workspace(
        "cycle",
        r#"{"tasks":[
            {"label":"a","command":"a","dependsOn":"b"},
            {"label":"b","command":"b","dependsOn":"a"}
        ]}"#,
    );
    let err = plan::build(&ws, "a", &ctx(&root)).unwrap_err();
    assert!(err.to_string().contains("cycle"), "{err}");
}

#[test]
fn a_missing_dependency_names_itself() {
    let (root, ws) = workspace(
        "missing-dep",
        r#"{"tasks":[{"label":"a","command":"a","dependsOn":"ghost"}]}"#,
    );
    let err = plan::build(&ws, "a", &ctx(&root)).unwrap_err();
    assert!(err.to_string().contains("ghost"), "{err}");
}

#[test]
fn hidden_tasks_stay_out_of_the_listing_but_still_run() {
    let (root, ws) = workspace(
        "hidden",
        r#"{"tasks":[
            {"label":"visible","command":"v","dependsOn":"secret"},
            {"label":"secret","command":"s","hide":true}
        ]}"#,
    );
    let listed: Vec<String> = ws.visible_tasks().iter().map(|t| t.label.clone()).collect();
    assert_eq!(listed, vec!["visible"]);
    let plan = plan::build(&ws, "visible", &ctx(&root)).unwrap();
    assert_eq!(plan.stages[0][0].label, "secret");
}

#[test]
fn tasks_without_a_command_are_rejected() {
    let (root, ws) = workspace(
        "npm-type",
        r#"{"tasks":[{"label":"a","type":"npm","script":"build"}]}"#,
    );
    let task = ws.find("a").unwrap();
    let err = resolve::resolve(&task, None, &ctx(&root)).unwrap_err();
    assert!(err.to_string().contains("no command"), "{err}");
}

#[test]
fn discovery_walks_up_to_the_nearest_workspace() {
    let (root, _) = workspace("discover", r#"{"tasks":[{"label":"a","command":"a"}]}"#);
    let deep = root.join("x").join("y");
    std::fs::create_dir_all(&deep).unwrap();
    let ws = Workspace::discover(&deep).unwrap();
    assert_eq!(ws.root, root.canonicalize().unwrap());
}

#[test]
fn presentation_hints_survive_for_a_frontend_to_use() {
    let (root, ws) = workspace(
        "presentation",
        r#"{"tasks":[{
            "label":"a","command":"a","isBackground":true,
            "presentation":{"panel":"dedicated","group":"servers"}
        }]}"#,
    );
    let task = ws.find("a").unwrap();
    let r = resolve::resolve(&task, None, &ctx(&root)).unwrap();
    assert!(r.is_background);
    assert_eq!(r.panel.as_deref(), Some("dedicated"));
    assert_eq!(r.group.as_deref(), Some("servers"));
}
