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
            builtin printf '\e]7777;shed;cmd;%s\a' "$(builtin printf '%s' "${BASH_REMATCH[1]}" | command base64 -w0)"
    fi
    builtin printf '\e]133;C\a'
}

# Runs first at every prompt: the last command's exit code, the directory.
__shed_precmd() {
    local status=$?
    builtin printf '\e]133;D;%s\a\e]7;file://%s%s\a' "$status" "${HOSTNAME:-}" "$PWD"
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
