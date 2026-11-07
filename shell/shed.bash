# Shed shell integration for bash.
#
# Shed loads this into every bash it starts. Elsewhere (over ssh, inside
# tmux), add to ~/.bashrc:   eval "$(shed init bash)"
#
# It tells Shed where prompts and commands begin and end (OSC 133), the
# working directory (OSC 7) and each command's text, so the keeper can keep
# the logbook. It also gives each tab its own history file, and binds
# Ctrl+Up / Ctrl+Down to walk the bench's history and Ctrl+Shift+Up /
# Ctrl+Shift+Down the global one. Plain Up stays bash's own.
#
# Commands bash leaves out of its history (with HISTCONTROL=ignorespace, the
# ones typed with a leading space) are left out of the logbook too.

[[ $- == *i* ]] || return 0
[[ -n ${SHED_PANE:-} && -z ${__shed_loaded:-} ]] || return 0
# PS0 and bind -x with READLINE_LINE need bash 4.4; macOS's own bash is 3.2.
((BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4))) || return 0
__shed_loaded=1

# Local history: one file per tab, shared by its panes. Bash reads HISTFILE
# after the startup files, so setting it here is enough.
if [[ -n ${SHED_TAB:-} ]]; then
    __shed_hist_dir="${SHED_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/shed}/hist"
    command mkdir -p "$__shed_hist_dir" 2>/dev/null
    HISTFILE="$__shed_hist_dir/$SHED_TAB"
fi

# Runs from PS0, after a command is read and before it runs. HISTCMD has
# moved past the number noted at the prompt only if bash saved the command.
__shed_preexec() {
    if ((HISTCMD > ${__shed_hist_next:-0})); then
        local line
        line=$(HISTTIMEFORMAT= builtin history 1)
        [[ $line =~ ^[[:space:]]*[0-9]+[[:space:]]+(.*)$ ]] &&
            builtin printf '\e]7777;shed;cmd;%s\a' "$(builtin printf '%s' "${BASH_REMATCH[1]}" | command base64 | command tr -d '\n')"
    fi
    builtin printf '\e]133;C\a'
}

# The shell's own aliases, as edited in Shed, are in SHED_SHELL_ALIASES (which
# ~/.bashrc sources too, once one is edited). When it changes it is read
# again, and the shell's aliases are sent to Shed again.
__shed_alias_names=()
__shed_shell_sync() {
    local file=${SHED_SHELL_ALIASES:-} content= name
    [[ -n $file && -r $file ]] && IFS= builtin read -r -d '' content <"$file"
    [[ -n ${__shed_shell_aliases+x} && $content == "${__shed_shell_last-}" ]] && return
    __shed_shell_last=$content
    for name in "${__shed_alias_names[@]}"; do builtin unalias "$name" 2>/dev/null; done
    builtin eval "${__shed_shell_aliases-}"
    builtin eval "$content"
    __shed_shell_aliases=$(builtin alias -p)
    builtin printf '\e]7777;shed;aliases;%s\a' "$(builtin printf '%s' "$__shed_shell_aliases" | command base64 | command tr -d '\n')"
    # Shed's own go back over them.
    unset __shed_alias_last
}
__shed_shell_sync

# Shed's aliases (global, bench, tab) come from a file the keeper keeps for
# this tab, one "name<TAB>command" per line. They are applied over the
# shell's own aliases; removing one brings the shell's version back.
__shed_alias_sync() {
    local file="${SHED_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/shed}/aliases/${SHED_TAB:-none}" content=
    [[ -r $file ]] && IFS= builtin read -r -d '' content <"$file"
    [[ $content == "${__shed_alias_last-}" ]] && return
    __shed_alias_last=$content
    local name value
    for name in "${__shed_alias_names[@]}"; do builtin unalias "$name" 2>/dev/null; done
    builtin eval "$__shed_shell_aliases"
    __shed_alias_names=()
    while IFS=$'\t' builtin read -r name value; do
        [[ -n $name ]] || continue
        builtin alias "$name=$value"
        __shed_alias_names+=("$name")
    done <<<"$content"
}

# Runs first at every prompt: the last command's exit code, the directory.
__shed_precmd() {
    local status=$?
    # Also clears kitty keyboard flags a crashed program may have left on:
    # readline reads keys the legacy way.
    builtin printf '\e]133;D;%s\a\e]7;file://%s%s\a\e[=0;1u' "$status" "${HOSTNAME:-}" "$PWD"
    __shed_shell_sync
    __shed_alias_sync
    builtin history -a
    __shed_hist_next=$HISTCMD
    return $status
}

# Runs last at every prompt, after any prompt tool has set PS1.
__shed_prompt_marks() {
    [[ $PS1 == *'133;A'* ]] || PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'
}

if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
    PROMPT_COMMAND=(__shed_precmd "${PROMPT_COMMAND[@]}" __shed_prompt_marks)
else
    PROMPT_COMMAND="__shed_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __shed_prompt_marks"
fi
PS0='$(__shed_preexec)'"${PS0:-}"

# Puts text on the line being edited, without running it. Used for walking
# history, and anything else that hands the shell a command to review.
__shed_set_line() {
    READLINE_LINE=$1
    READLINE_POINT=${#READLINE_LINE}
}

# Walks a scope's history: $1 is bench or global, $2 is 1 (older) or -1.
# What was typed before the walk began is the prefix to match.
__shed_walk() {
    local scope=$1 step=$2
    if [[ $scope != "${__shed_walk_scope:-}" || $READLINE_LINE != "${__shed_walk_shown:-}" ]]; then
        __shed_walk_scope=$scope
        __shed_walk_pos=0
        __shed_walk_prefix=$READLINE_LINE
    fi
    local pos=$((__shed_walk_pos + step))
    if ((pos <= 0)); then
        __shed_set_line "$__shed_walk_prefix"
        __shed_walk_scope= __shed_walk_shown=
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
    __shed_walk_pos=$got
    __shed_set_line "${out#*$'\n'}"
    __shed_walk_shown=$READLINE_LINE
    builtin printf '\e]7777;shed;walk;%s;%s;%s;%s\a' "$scope" "$got" "$total" "$source"
}

builtin bind -x '"\e[1;5A": __shed_walk bench 1'
builtin bind -x '"\e[1;5B": __shed_walk bench -1'
builtin bind -x '"\e[1;6A": __shed_walk global 1'
builtin bind -x '"\e[1;6B": __shed_walk global -1'

# The AI prompt: Enter on a line starting with ">" hands it to Shed, which
# asks the AI and puts the command it answers with on the next prompt, to
# review. Enter runs a check first, then accepts the line, which the check
# emptied if it took it. Ctrl+J too: an Enter typed while a command still
# runs reaches readline as one, and must not run "> words" as a redirection.
# Only when Shed's AI prompt is on (SHED_AI).
if [[ -n ${SHED_AI:-} ]]; then
    __shed_ask() {
        [[ $READLINE_LINE == '>'* ]] || return
        builtin printf '\e]7777;shed;ai;%s\a' "$(builtin printf '%s' "${READLINE_LINE#>}" | command base64 | command tr -d '\n')"
        builtin history -s -- "$READLINE_LINE"
        READLINE_LINE=
    }
    builtin bind -x '"\C-x\C-_a": __shed_ask'
    builtin bind '"\C-x\C-_e": accept-line'
    builtin bind '"\C-m": "\C-x\C-_a\C-x\C-_e"'
    builtin bind '"\C-j": "\C-x\C-_a\C-x\C-_e"'
fi
