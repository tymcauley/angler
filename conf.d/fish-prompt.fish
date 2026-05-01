# ---- defaults: any of these can be overridden in config.fish via `set -g`. ----
# Set unconditionally (above the interactive gate) so tests and tooling that
# source this file in scripted mode also get the defaults.
#
# Symbols.
set -q _fp_symbol_modified;  or set -g _fp_symbol_modified  '*'
set -q _fp_symbol_staged;    or set -g _fp_symbol_staged    '+'
set -q _fp_symbol_untracked; or set -g _fp_symbol_untracked '?'
set -q _fp_symbol_conflict;  or set -g _fp_symbol_conflict  '!'
set -q _fp_symbol_unknown;   or set -g _fp_symbol_unknown   '?'
set -q _fp_symbol_ahead;     or set -g _fp_symbol_ahead     '↑'
set -q _fp_symbol_behind;    or set -g _fp_symbol_behind    '↓'
set -q _fp_symbol_gone;      or set -g _fp_symbol_gone      '↯'
set -q _fp_symbol_stash;     or set -g _fp_symbol_stash     '≡'
set -q _fp_symbol_prompt;    or set -g _fp_symbol_prompt    '❯'

# Vi-mode-aware prompt symbol (line 2). Used when $fish_bind_mode != insert
# and _fp_show_vi_mode = 1. Insert mode falls through to _fp_symbol_prompt.
set -q _fp_symbol_vi_default; or set -g _fp_symbol_vi_default '❮'
set -q _fp_symbol_vi_visual;  or set -g _fp_symbol_vi_visual  'V'
set -q _fp_symbol_vi_replace; or set -g _fp_symbol_vi_replace 'R'

# Colors. Stored as lists so multi-arg styles like `red --bold` work directly
# when expanded into `set_color`.
set -q _fp_color_path;      or set -g _fp_color_path      cyan
set -q _fp_color_branch;    or set -g _fp_color_branch    yellow
set -q _fp_color_operation; or set -g _fp_color_operation magenta
set -q _fp_color_modified;  or set -g _fp_color_modified  red
set -q _fp_color_staged;    or set -g _fp_color_staged    green
set -q _fp_color_untracked; or set -g _fp_color_untracked yellow
set -q _fp_color_conflict;  or set -g _fp_color_conflict  red --bold
set -q _fp_color_unknown;   or set -g _fp_color_unknown   yellow
set -q _fp_color_ahead;     or set -g _fp_color_ahead     yellow
set -q _fp_color_behind;    or set -g _fp_color_behind    yellow
set -q _fp_color_gone;      or set -g _fp_color_gone      red
set -q _fp_color_stash;     or set -g _fp_color_stash     blue
set -q _fp_color_exit_code; or set -g _fp_color_exit_code red
set -q _fp_color_time;      or set -g _fp_color_time      brblack
set -q _fp_color_duration;  or set -g _fp_color_duration  yellow
set -q _fp_color_ssh;       or set -g _fp_color_ssh       red --bold
set -q _fp_color_venv;      or set -g _fp_color_venv      blue
set -q _fp_color_direnv;    or set -g _fp_color_direnv    green
set -q _fp_color_vi_default; or set -g _fp_color_vi_default green
set -q _fp_color_vi_visual;  or set -g _fp_color_vi_visual  magenta
set -q _fp_color_vi_replace; or set -g _fp_color_vi_replace red

# Toggles (1 = show, anything else = hide).
set -q _fp_show_ahead_behind;       or set -g _fp_show_ahead_behind       1
set -q _fp_show_stash;              or set -g _fp_show_stash              1
set -q _fp_show_operation;          or set -g _fp_show_operation          1
set -q _fp_show_exit_code;          or set -g _fp_show_exit_code          1
set -q _fp_show_time;               or set -g _fp_show_time               1
set -q _fp_show_cmd_duration;       or set -g _fp_show_cmd_duration       1
set -q _fp_show_vi_mode;            or set -g _fp_show_vi_mode            0
set -q _fp_show_ssh;                or set -g _fp_show_ssh                1
set -q _fp_show_venv;               or set -g _fp_show_venv               1
set -q _fp_show_direnv;             or set -g _fp_show_direnv             1
set -q _fp_cmd_duration_threshold_ms; or set -g _fp_cmd_duration_threshold_ms 1000

# Daemon tuning.
set -q _fp_dirty_deadline_ms; or set -g _fp_dirty_deadline_ms 200

# ---- runtime state and daemon spawn ----
status is-interactive; or exit 0
command -q fish-prompt-daemon; or exit 0

set -g _fp_dir (command mktemp -d -t fish-prompt-$fish_pid)
set -g _fp_status_file $_fp_dir/status
set -g _fp_request_fifo $_fp_dir/req

command mkfifo $_fp_request_fifo

# Daemon opens the FIFO with O_RDWR (non-blocking) and exits when its parent
# (this fish) dies, via a getppid() watchdog. So fish doesn't need to hold a
# long-lived fd open.
command fish-prompt-daemon \
    --fish-pid $fish_pid \
    --status-file $_fp_status_file \
    --request-fifo $_fp_request_fifo \
    --dirty-deadline-ms $_fp_dirty_deadline_ms &
disown

function _fp_request_status --on-variable PWD
    echo $PWD >$_fp_request_fifo
end

function _fp_repaint --on-signal SIGUSR1
    commandline -f repaint
end

function _fp_cleanup --on-event fish_exit
    command rm -rf $_fp_dir
end

# Trigger an initial request for the starting directory. Background it: if the
# daemon isn't ready yet, the open(O_WRONLY) on the FIFO would block.
echo $PWD >$_fp_request_fifo &
disown 2>/dev/null
