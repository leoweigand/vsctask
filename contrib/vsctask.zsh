# A taste of the zsh frontend. Source it, don't install it:
#   source ~/git/vsctask/contrib/vsctask.zsh
# Then `vt` picks a task and runs it, `vte` picks one and drops the command
# on your prompt so you can edit it before running.

_vt_labels() { vsctask list --json | jq -r '.[].label' }

vt() {
  local label
  label=$(_vt_labels | fzf --height 40% --reverse --prompt='task> ') || return
  [[ -n $label ]] && vsctask run "$label"
}

vte() {
  local label
  label=$(_vt_labels | fzf --height 40% --reverse --prompt='task> ') || return
  [[ -z $label ]] && return
  print -z "(cd $(vsctask show "$label" --json | jq -r .cwd) && $(vsctask emit "$label"))"
}

# Completion for the real CLI.
_vsctask() {
  local -a labels
  labels=(${(f)"$(_vt_labels 2>/dev/null)"})
  _arguments '1:command:(list show emit plan run)' "*:task:($labels)"
}
compdef _vsctask vsctask
