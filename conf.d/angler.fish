# ---- defaults: any of these can be overridden in config.fish via `set -g`. ----
# Set unconditionally (above the interactive gate) so tests and tooling that
# source this file in scripted mode also get the defaults.
#
# Symbols.
set -q _angler_symbol_modified;  or set -g _angler_symbol_modified  '*'
set -q _angler_symbol_staged;    or set -g _angler_symbol_staged    '+'
set -q _angler_symbol_untracked; or set -g _angler_symbol_untracked '?'
set -q _angler_symbol_conflict;  or set -g _angler_symbol_conflict  '!'
set -q _angler_symbol_unknown;   or set -g _angler_symbol_unknown   '?'
set -q _angler_symbol_ahead;     or set -g _angler_symbol_ahead     '↑'
set -q _angler_symbol_behind;    or set -g _angler_symbol_behind    '↓'
set -q _angler_symbol_gone;      or set -g _angler_symbol_gone      '↯'
set -q _angler_symbol_stash;     or set -g _angler_symbol_stash     '≡'
set -q _angler_symbol_submodule; or set -g _angler_symbol_submodule 's'
set -q _angler_symbol_prompt;    or set -g _angler_symbol_prompt    '❯'

# Vi mode indicator strings. Rendered by fish_mode_prompt (left of line 1)
# when $fish_key_bindings = fish_vi_key_bindings. The line-2 prompt symbol
# stays mode-agnostic. Defaults are reverse-video blocks: a letter padded
# with spaces, drawn with the configured color used as the background (via
# `--reverse`) so the terminal's default foreground shows through. To get
# plain colored letters back, drop `--reverse` from the color knobs; for
# the older bracket style, set the symbols to '[I]'/etc.
set -q _angler_symbol_vi_default; or set -g _angler_symbol_vi_default ' N '
set -q _angler_symbol_vi_insert;  or set -g _angler_symbol_vi_insert  ' I '
set -q _angler_symbol_vi_visual;  or set -g _angler_symbol_vi_visual  ' V '
set -q _angler_symbol_vi_replace; or set -g _angler_symbol_vi_replace ' R '

# Colors. Stored as lists so multi-arg styles like `red --bold` work directly
# when expanded into `set_color`.
# Path is rendered split: prefix (truncated dirs) in _angler_color_path, the last
# component in _angler_color_path_tail — emphasis on the directory you actually
# care about. Both default to cyan; the tail is bold.
set -q _angler_color_path;      or set -g _angler_color_path      cyan
set -q _angler_color_path_tail; or set -g _angler_color_path_tail cyan --bold

# Most colored elements default to bold so the overall weight reads consistent;
# time and duration intentionally stay plain so they read as background metadata.
set -q _angler_color_branch;    or set -g _angler_color_branch    yellow --bold
set -q _angler_color_operation; or set -g _angler_color_operation magenta --bold
set -q _angler_color_modified;  or set -g _angler_color_modified  red --bold
set -q _angler_color_staged;    or set -g _angler_color_staged    green --bold
set -q _angler_color_untracked; or set -g _angler_color_untracked yellow --bold
set -q _angler_color_conflict;  or set -g _angler_color_conflict  red --bold
set -q _angler_color_unknown;   or set -g _angler_color_unknown   yellow --bold
set -q _angler_color_ahead;     or set -g _angler_color_ahead     yellow --bold
set -q _angler_color_behind;    or set -g _angler_color_behind    yellow --bold
set -q _angler_color_gone;      or set -g _angler_color_gone      red --bold
set -q _angler_color_stash;     or set -g _angler_color_stash     blue --bold
set -q _angler_color_submodule; or set -g _angler_color_submodule yellow --bold
set -q _angler_color_exit_code; or set -g _angler_color_exit_code red --bold
set -q _angler_color_time;      or set -g _angler_color_time      brblack
set -q _angler_color_duration;  or set -g _angler_color_duration  brblack
set -q _angler_color_ssh;       or set -g _angler_color_ssh       red --bold
set -q _angler_color_venv;      or set -g _angler_color_venv      blue --bold
set -q _angler_color_direnv;    or set -g _angler_color_direnv    green --bold
set -q _angler_color_prompt;    or set -g _angler_color_prompt    green --bold
set -q _angler_color_vi_default; or set -g _angler_color_vi_default red --reverse --bold
set -q _angler_color_vi_insert;  or set -g _angler_color_vi_insert  green --reverse --bold
set -q _angler_color_vi_visual;  or set -g _angler_color_vi_visual  magenta --reverse --bold
set -q _angler_color_vi_replace; or set -g _angler_color_vi_replace yellow --reverse --bold

# Toggles (1 = show, anything else = hide).
set -q _angler_show_ahead_behind;       or set -g _angler_show_ahead_behind       1
set -q _angler_show_stash;              or set -g _angler_show_stash              1
set -q _angler_show_submodule;          or set -g _angler_show_submodule          1
set -q _angler_show_operation;          or set -g _angler_show_operation          1
set -q _angler_show_exit_code;          or set -g _angler_show_exit_code          1
set -q _angler_show_time;               or set -g _angler_show_time               1
set -q _angler_show_cmd_duration;       or set -g _angler_show_cmd_duration       1
set -q _angler_show_vi_mode;            or set -g _angler_show_vi_mode            1
set -q _angler_show_ssh;                or set -g _angler_show_ssh                1
set -q _angler_show_venv;               or set -g _angler_show_venv               1
set -q _angler_show_direnv;             or set -g _angler_show_direnv             1
set -q _angler_cmd_duration_threshold_ms; or set -g _angler_cmd_duration_threshold_ms 1000

# Daemon tuning.
set -q _angler_dirty_deadline_ms; or set -g _angler_dirty_deadline_ms 200

# Optional log file for the daemon (off by default). Set this in config.fish
# to enable; the path may include $fish_pid for a per-shell log, or be a
# fixed path that all shells share (each line is prefixed with the daemon's
# PID for disambiguation).
set -q _angler_log_file; or set -g _angler_log_file ""

# ---- runtime state and daemon spawn ----
status is-interactive; or exit 0

# _angler_init is autoloaded from functions/. It's lazy: deferred to the
# first request from fish_prompt's per-render kick, so config.fish has run
# by then and PATH manipulations there are visible (`command -q
# angler-daemon` works regardless of where the binary lives).

# _angler_ensure_daemon respawns the daemon if it has died. Without this, a dead
# daemon leaves the FIFO with no reader, and the write below blocks on
# open(O_WRONLY) — i.e., the next cd hangs the shell. The write is also
# backgrounded so the rare respawn race (fish writes before the new daemon
# has opened the FIFO) can't stall fish either.
#
# Wire framing: `AN1\0<path>\0` — a wire-version sentinel followed by the
# request path, both NUL-terminated. NUL framing keeps embedded newlines
# and non-UTF-8 bytes intact (matches `find -print0`). The daemon rejects
# any first token that isn't `AN1`, so old fish + new daemon (or vice
# versa) degrades to "no git block" instead of silently misparsing.
function _angler_request_status --on-variable PWD
    _angler_init
    set -q _angler_init_ok; or return
    _angler_ensure_daemon
    # External printf (not the builtin) so $last_pid gets set — fish
    # leaves it empty for backgrounded builtin jobs, and bare `disown`
    # would then fall back to fish's "last constructed job", which is
    # the user's, not ours.
    command printf 'AN1\0%s\0' $PWD >$_angler_request_fifo &
    disown $last_pid 2>/dev/null
end

function _angler_repaint --on-signal SIGUSR1
    commandline -f repaint
end

# Mostly redundant with the daemon's parent-death cleanup, which removes
# the same dir on its way out. Kept because fish_exit fires synchronously
# in fish's exit path, so on a normal exit the dir is gone before the
# daemon's signal-handler thread races to do the same. fish_exit does NOT
# fire on `exec fish` (correctly — the new fish image adopts the dir) or
# on kill -9 (the daemon-side cleanup catches that case).
function _angler_cleanup --on-event fish_exit
    set -q _angler_dir; and command rm -rf $_angler_dir
end
