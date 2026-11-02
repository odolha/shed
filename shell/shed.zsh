# Shed shell integration for zsh.
#
# Shed loads this into every zsh it starts. Elsewhere (over ssh, inside
# tmux), add to the end of ~/.zshrc:   eval "$(shed init zsh)"
#
# It tells Shed where prompts and commands begin and end (OSC 133), the
# working directory (OSC 7) and each command's text, so the keeper can keep
# the logbook. It also gives each tab its own history file, and binds
# Ctrl+Up / Ctrl+Down to walk the bench's history and Ctrl+Shift+Up /
# Ctrl+Shift+Down the global one. Plain Up stays zsh's own.
#
# With HIST_IGNORE_SPACE, commands typed with a leading space are left out
# of the logbook too.

[[ -o interactive ]] || return 0
[[ -n ${SHED_PANE:-} && -z ${__shed_loaded:-} ]] || return 0
typeset -g __shed_loaded=1

builtin autoload -Uz add-zsh-hook

typeset -g __shed_state_dir="${SHED_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/shed}"

__shed_b64() {
    builtin printf '%s' "$1" | command base64 | command tr -d '\n'
}

# Runs after a command is read and before it runs.
__shed_preexec() {
    if [[ ! ( -o histignorespace && $1 == ' '* ) ]]; then
        builtin printf '\e]7777;shed;cmd;%s\a' "$(__shed_b64 "$1")"
    fi
    builtin printf '\e]133;C\a'
}

# Shed's aliases (global, bench, tab) come from a file the keeper keeps for
# this tab, one "name<TAB>command" per line. They are applied over the
# shell's own aliases; removing one brings the shell's version back. The
# shell's own are read at the first prompt, once ~/.zshrc has made them.
typeset -ga __shed_alias_names=()
__shed_alias_sync() {
    if (( ! ${+__shed_shell_aliases} )); then
        typeset -g __shed_shell_aliases="$(builtin alias -rL)"
        builtin printf '\e]7777;shed;aliases;%s\a' "$(__shed_b64 "$__shed_shell_aliases")"
    fi
    local file="$__shed_state_dir/aliases/${SHED_TAB:-none}" content=
    [[ -r $file ]] && content="$(<"$file")"
    [[ $content == "${__shed_alias_last-}" ]] && return
    typeset -g __shed_alias_last=$content
    local name value line
    for name in "${__shed_alias_names[@]}"; do builtin unalias -- "$name" 2>/dev/null; done
    builtin eval "$__shed_shell_aliases"
    __shed_alias_names=()
    for line in "${(@f)content}"; do
        name=${line%%$'\t'*}
        value=${line#*$'\t'}
        [[ -n $name && $line == *$'\t'* ]] || continue
        builtin alias -- "$name=$value"
        __shed_alias_names+=("$name")
    done
}

# Runs first at every prompt: the last command's exit code, the directory.
__shed_precmd() {
    local ret=$?
    # Also clears kitty keyboard flags a crashed program may have left on:
    # the line editor reads keys the legacy way.
    builtin printf '\e]133;D;%s\a\e]7;file://%s%s\a\e[=0;1u' "$ret" "${HOST:-}" "$PWD"
    __shed_alias_sync
    return $ret
}

# Runs last at every prompt, after any prompt tool has set PS1.
__shed_prompt_marks() {
    [[ $PS1 == *'133;A'* ]] || PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
}

# The first prompt comes after ~/.zshrc, so what it set up is in place:
# the tab's own history replaces the one it loaded, and the marks go on
# after every other prompt hook.
__shed_first_prompt() {
    add-zsh-hook -d precmd __shed_first_prompt
    if [[ -n ${SHED_TAB:-} ]]; then
        command mkdir -p "$__shed_state_dir/hist" 2>/dev/null
        builtin fc -p "$__shed_state_dir/hist/$SHED_TAB" "${HISTSIZE:-10000}" "${SAVEHIST:-10000}"
    fi
    add-zsh-hook -d precmd __shed_prompt_marks
    add-zsh-hook precmd __shed_prompt_marks
    __shed_prompt_marks
}

precmd_functions=(__shed_precmd ${precmd_functions[@]:#__shed_precmd})
add-zsh-hook precmd __shed_first_prompt
add-zsh-hook precmd __shed_prompt_marks
add-zsh-hook preexec __shed_preexec

# Puts text on the line being edited, without running it.
__shed_set_line() {
    BUFFER=$1
    CURSOR=${#BUFFER}
}

# Walks a scope's history: $1 is bench or global, $2 is 1 (older) or -1.
# What was typed before the walk began is the prefix to match.
__shed_walk() {
    local scope=$1 step=$2
    if [[ $scope != "${__shed_walk_scope:-}" || $BUFFER != "${__shed_walk_shown:-}" ]]; then
        typeset -g __shed_walk_scope=$scope __shed_walk_pos=0 __shed_walk_prefix=$BUFFER
    fi
    local pos=$((__shed_walk_pos + step))
    if ((pos <= 0)); then
        __shed_set_line "$__shed_walk_prefix"
        typeset -g __shed_walk_scope= __shed_walk_shown=
        builtin printf '\e]7777;shed;walk;end\a'
        return
    fi
    local out
    if ! out=$("${SHED_BIN:-shed}" hook walk --scope "$scope" --pos "$pos" --prefix "$__shed_walk_prefix" 2>/dev/null); then
        builtin printf '\e]7777;shed;walk;%s;0;0;\a' "$scope"
        return
    fi
    # First line: "<pos> <total> <source tab title, base64>"; then the command.
    local head=${out%%$'\n'*} got total source
    builtin read -r got total source <<<"$head"
    typeset -g __shed_walk_pos=$got
    __shed_set_line "${out#*$'\n'}"
    typeset -g __shed_walk_shown=$BUFFER
    builtin printf '\e]7777;shed;walk;%s;%s;%s;%s\a' "$scope" "$got" "$total" "$source"
}

__shed_walk_bench_older() { __shed_walk bench 1 }
__shed_walk_bench_newer() { __shed_walk bench -1 }
__shed_walk_global_older() { __shed_walk global 1 }
__shed_walk_global_newer() { __shed_walk global -1 }
zle -N __shed_walk_bench_older
zle -N __shed_walk_bench_newer
zle -N __shed_walk_global_older
zle -N __shed_walk_global_newer
builtin bindkey '\e[1;5A' __shed_walk_bench_older
builtin bindkey '\e[1;5B' __shed_walk_bench_newer
builtin bindkey '\e[1;6A' __shed_walk_global_older
builtin bindkey '\e[1;6B' __shed_walk_global_newer
