# Shell integration for the terminal: emits OSC 133 "semantic prompt" marks so
# the terminal can capture command blocks (command + output + exit code).
#
# Enable it by sourcing this file from your ~/.zshrc, e.g.:
#     source /path/to/terminal/shell-integration.zsh
#
# Marks emitted (per the FinalTerm / iTerm2 convention):
#   OSC 133;A          prompt start
#   OSC 133;B          command start (end of prompt)
#   OSC 133;C          output start (command is about to run)
#   OSC 133;D;<code>   command end, with the exit status
#   OSC 7;file://host/cwd   current working directory

# Load once (auto-injection + a manual `source` must not double-install).
[[ -n $__TERM_INTEGRATION_LOADED ]] && return
__TERM_INTEGRATION_LOADED=1

# Emit a raw escape (BEL-terminated OSC).
__term_osc() { printf '\033]%s\007' "$1"; }

# Before each prompt: report the previous command's exit, then mark prompt start
# and the cwd.
__term_precmd() {
  local exit=$?
  if [[ -n $__term_command_running ]]; then
    __term_osc "133;D;$exit"
    unset __term_command_running
  fi
  __term_osc "133;A"
  __term_osc "7;file://${HOST}${PWD}"
}

# Just before running a command: mark output start and send the exact command
# line (base64-encoded so any characters survive the OSC channel). This is more
# robust than reading the command off the screen for themed/multi-line prompts.
__term_preexec() {
  __term_command_running=1
  local enc
  enc=$(print -rn -- "$1" | base64 | tr -d '\n')
  __term_osc "133;C;${enc}"
}

# Mark command start at the END of the prompt (where you start typing), so the
# command capture excludes the prompt itself. %{...%} = zero-width to zsh.
__term_install_prompt() {
  PS1="$PS1%{$(__term_osc '133;B')%}"
}

autoload -Uz add-zsh-hook 2>/dev/null
if (( $+functions[add-zsh-hook] )); then
  add-zsh-hook precmd __term_precmd
  add-zsh-hook preexec __term_preexec
fi
__term_install_prompt
