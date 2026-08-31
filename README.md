# vsctask

Run the tasks from a VS Code `tasks.json` without opening VS Code.

You already wrote down how to build, test and lint your project — it's just
stuck inside `.vscode/tasks.json` where only the editor can see it. This tool
reads that file, resolves the variables and dependencies the way VS Code
would, and runs the result in your terminal.

It handles JSONC, `shell` and `process` tasks, VS Code's quoting rules,
per-platform `options`, and most `${variables}`. Provider tasks like `npm` or
`gulp` come from editor extensions, so those it can't do.

## The CLI

```bash
cargo build --release
```

```
vsctask list          # what's in the file
vsctask show <label>  # resolved command, cwd and env
vsctask emit <label>  # just the shell line, ready to paste
vsctask plan <label>  # dependsOn flattened into stages
vsctask run <label>   # run it, dependencies first
```

`list`, `show` and `plan` take `--json`, which is also the seam for building
other frontends on top. A small zsh function lives in `contrib/`.

## The Herdr plugin

The repo doubles as a Herdr plugin: an `fzf` picker in a popup that runs the
chosen task in a pane. It needs `fzf` on the PATH, and Rust to build. Install
it straight from GitHub, then bind a key:

```bash
herdr plugin install leoweigand/vsctask
```

(For hacking on it, clone and `herdr plugin link /path/to/vsctask` instead —
link skips the build step, so run `cargo build --release` yourself.)

```toml
# ~/.config/herdr/config.toml
[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "vsctask.pick"
description = "run a task"
```

Then `herdr server reload-config` and press `prefix+t` in a pane.

That's it, really.

## Licence

MIT. See [LICENSE](LICENSE).
