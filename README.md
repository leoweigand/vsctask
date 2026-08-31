# vsctask

Run the tasks from a VS Code `tasks.json` without opening VS Code.

You already wrote down how to build, test and lint your project — it's just
stuck inside `.vscode/tasks.json` where only the editor can see it. This tool
reads that file, resolves the variables and dependencies the way VS Code
would, and runs the result in your terminal.

```
vsctask list          # what's in the file
vsctask show <label>  # resolved command, cwd and env
vsctask emit <label>  # just the shell line, ready to paste
vsctask plan <label>  # dependsOn flattened into stages
vsctask run <label>   # run it, dependencies first
```

`list`, `show` and `plan` take `--json`, which is also the seam for building
other frontends on top. There's one included: a Herdr
plugin that pops up an `fzf` picker and runs the chosen task in a pane
(`herdr-plugin.toml`), plus a small zsh function in `contrib/`.

It handles JSONC, `shell` and `process` tasks, VS Code's quoting rules,
per-platform `options`, and most `${variables}`. Provider tasks like `npm` or
`gulp` come from editor extensions, so those it can't do.

```bash
cargo build --release
```

That's it, really.
