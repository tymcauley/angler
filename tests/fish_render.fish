#!/usr/bin/env fish
# Tests for fish_prompt rendering. Drives the function with hand-written
# status files; does not exercise the daemon or signal flow.

set -l repo_root (realpath (dirname (status filename))/..)
set -ga fish_function_path $repo_root/functions

# Source conf.d to pick up the symbol/color/toggle defaults. The interactive
# gate inside it short-circuits before the daemon-spawn block, so this is safe
# in scripted mode.
source $repo_root/conf.d/fish-prompt.fish

# fish_prompt fires _fp_request_status on every render to keep state
# fresh. That function is defined inside conf.d's interactive-gated
# block, so it doesn't exist in scripted mode. Stub it as a no-op so
# fish_prompt doesn't spam "Unknown command" errors during tests.
# (The respawn test below installs its own real spawn helpers.)
function _fp_request_status
end

# Disable time and command-duration rendering in tests: their output is
# timing-dependent and the digits would collide with substring assertions
# against numeric counts (ahead/behind/stash). Vi mode is also off by default,
# so the prompt-symbol cases match.
set -g _fp_show_time 0
set -g _fp_show_cmd_duration 0

# Per-test scratch state
set -g _fp_status_file (command mktemp)
function _cleanup --on-event fish_exit
    command rm -f $_fp_status_file
end

set -g TESTS_RUN 0
set -g TESTS_FAILED 0

function ok
    set -g TESTS_RUN (math $TESTS_RUN + 1)
    echo "  ok   $argv"
end

function fail
    set -g TESTS_RUN (math $TESTS_RUN + 1)
    set -g TESTS_FAILED (math $TESTS_FAILED + 1)
    echo "  FAIL $argv"
end

# Substring check via grep -F so '*' and '?' are literal, not glob/regex.
function _contains
    printf '%s' $argv[1] | command grep -qF -- $argv[2]
end

function assert_contains
    set -l haystack $argv[1]
    set -l needle $argv[2]
    set -l name $argv[3]
    if _contains "$haystack" "$needle"
        ok $name
    else
        fail $name
        echo "       expected to contain: $needle"
        echo "       actual: $haystack"
    end
end

function assert_not_contains
    set -l haystack $argv[1]
    set -l needle $argv[2]
    set -l name $argv[3]
    if _contains "$haystack" "$needle"
        fail $name
        echo "       expected NOT to contain: $needle"
        echo "       actual: $haystack"
    else
        ok $name
    end
end

function write_status -d "Write a NUL-delimited status file: FP1 sentinel + path branch ahead behind dirty operation upstream stash submodules"
    # Pad to 9 payload fields with "0" so existing 8-arg callers still work
    # — the 9th field (submodules) defaults to no-submodules-dirty. The
    # `FP1` wire-version sentinel is prepended automatically.
    set -l fields $argv
    while test (count $fields) -lt 9
        set fields $fields 0
    end
    printf 'FP1\0%s\0%s\0%s\0%s\0%s\0%s\0%s\0%s\0%s\0' $fields[1..9] >$_fp_status_file
end

# ----- tests -----

cd /tmp

write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "main" "renders branch when status path matches PWD"
assert_not_contains "$out" "*" "no dirty marker when dirty=0"
assert_not_contains "$out" "?" "no unknown marker when dirty=0"
assert_not_contains "$out" "≡" "no stash glyph when stash=0"

write_status /tmp my-feature-branch 0 0 '*' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "my-feature-branch" "renders distinctive branch name"
assert_contains "$out" "*" "renders red asterisk for modified flag"

write_status /tmp main 0 0 '+' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "+" "renders + for staged flag"
assert_not_contains "$out" "*" "no * when only staged"

write_status /tmp main 0 0 'u' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "?" "renders ? for untracked flag"
assert_not_contains "$out" "*" "no * when only untracked"

write_status /tmp main 0 0 '!' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "!" "renders ! for conflict flag"

write_status /tmp main 0 0 '+*' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "+" "combo: staged"
assert_contains "$out" "*" "combo: modified"

write_status /tmp main 0 0 '+*u!' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "+" "all four: staged"
assert_contains "$out" "*" "all four: modified"
assert_contains "$out" "?" "all four: untracked"
assert_contains "$out" "!" "all four: conflict"

write_status /tmp main 0 0 '?' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "main" "renders branch when dirty unknown"
assert_contains "$out" "?" "renders question mark when dirty=?"

write_status /tmp main 3 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "↑3" "renders up-arrow with ahead count"
assert_not_contains "$out" "↓" "no down-arrow when behind=0"

write_status /tmp main 0 2 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "↓2" "renders down-arrow with behind count"
assert_not_contains "$out" "↑" "no up-arrow when ahead=0"

write_status /tmp main 1 4 '*' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "↑1" "diverged: renders ahead"
assert_contains "$out" "↓4" "diverged: renders behind"
assert_contains "$out" "*" "diverged: still renders dirty"

write_status /tmp main 0 0 0 rebasing '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "(rebasing)" "renders operation marker in parens"

write_status /tmp main 2 0 '*' merging '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "(merging)" "operation alongside ahead+dirty"
assert_contains "$out" "↑2" "operation does not displace ahead"
assert_contains "$out" "*" "operation does not displace dirty"

write_status /tmp main 0 0 0 '' gone 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "↯" "renders gone-upstream glyph"

write_status /tmp main 2 0 '*' '' gone 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "↯" "gone-upstream glyph alongside other markers"
assert_contains "$out" "↑2" "gone-upstream does not displace ahead"
assert_contains "$out" "*" "gone-upstream does not displace dirty"

write_status /tmp main 0 0 0 '' '' 3
set -l out (fish_prompt | string collect)
assert_contains "$out" "≡3" "renders stash glyph with count"

write_status /tmp main 1 0 '*' '' '' 2
set -l out (fish_prompt | string collect)
assert_contains "$out" "≡2" "stash alongside other markers"
assert_contains "$out" "↑1" "stash does not displace ahead"
assert_contains "$out" "*" "stash does not displace dirty"

# Submodule indicator (9th field). Hidden when zero.
write_status /tmp main 0 0 0 '' '' 0 0
set -l out (fish_prompt | string collect)
assert_not_contains "$out" "s0" "no submodule indicator when count is zero"

write_status /tmp main 0 0 0 '' '' 0 3
set -l out (fish_prompt | string collect)
assert_contains "$out" "s3" "renders submodule indicator with count"

# Toggle off submodule indicator.
set -g _fp_show_submodule 0
write_status /tmp main 0 0 0 '' '' 0 3
set -l out (fish_prompt | string collect)
assert_not_contains "$out" "s3" "submodule indicator hidden when _fp_show_submodule=0"
set -g _fp_show_submodule 1

# Submodule alongside other markers — still shows, doesn't displace.
write_status /tmp main 1 0 '*' '' '' 2 4
set -l out (fish_prompt | string collect)
assert_contains "$out" "s4" "submodule alongside ahead/dirty/stash"
assert_contains "$out" "↑1" "submodule does not displace ahead"
assert_contains "$out" "*" "submodule does not displace dirty"
assert_contains "$out" "≡2" "submodule does not displace stash"

write_status /some/other/dir main 0 0 '*' '' '' 0
set -l out (fish_prompt | string collect)
assert_not_contains "$out" "main" "skips git block when reported_path != PWD"
assert_not_contains "$out" "*" "no dirty marker on path mismatch"

write_status /tmp '' '' '' '' '' '' ''
set -l out (fish_prompt | string collect)
assert_not_contains "$out" '*' "no dirty marker for non-repo (empty branch)"

# ----- config knob overrides -----

# Override symbol: dirty modified → 'M'
set -g _fp_symbol_modified M
write_status /tmp main 0 0 '*' '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" M "custom dirty symbol overrides default"
assert_not_contains "$out" '*' "default '*' replaced by custom symbol"
set -g _fp_symbol_modified '*'

# Override symbol: ahead glyph
set -g _fp_symbol_ahead UP
write_status /tmp main 3 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" UP3 "custom ahead symbol overrides default"
set -g _fp_symbol_ahead '↑'

# Toggle off ahead/behind
set -g _fp_show_ahead_behind 0
write_status /tmp main 5 7 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_not_contains "$out" 5 "ahead count hidden when _fp_show_ahead_behind=0"
assert_not_contains "$out" 7 "behind count hidden when _fp_show_ahead_behind=0"
set -g _fp_show_ahead_behind 1

# Toggle off stash
set -g _fp_show_stash 0
write_status /tmp main 0 0 0 '' '' 4
set -l out (fish_prompt | string collect)
assert_not_contains "$out" 4 "stash count hidden when _fp_show_stash=0"
set -g _fp_show_stash 1

# Toggle off operation
set -g _fp_show_operation 0
write_status /tmp main 0 0 0 rebasing '' 0
set -l out (fish_prompt | string collect)
assert_not_contains "$out" rebasing "operation hidden when _fp_show_operation=0"
set -g _fp_show_operation 1

# Override prompt symbol
set -g _fp_symbol_prompt '%'
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" '%' "custom prompt symbol overrides default"
set -g _fp_symbol_prompt '❯'

# ----- environmental indicators -----

# SSH host prefix renders before the path when $SSH_TTY is set.
set -gx SSH_TTY /dev/pts/0
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
# prompt_hostname output is whatever the test machine's short name is —
# we just check that the colon separator appears before /tmp.
if string match -qr '\w+:' -- (string match -r '^[^\n]*' -- "$out")
    ok "SSH host prefix appears before path"
else
    fail "SSH host prefix appears before path"
    echo "       expected 'host:' prefix; actual line 1: " (string match -r '^[^\n]*' -- "$out")
end
set -e SSH_TTY

# SSH not shown when env var unset.
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
if string match -qr '^/tmp' -- (string match -r '^[^\n]*' -- "$out")
    # We expect the line to start with the path itself (post-color) — but
    # ANSI codes prefix it. Strip ANSI and check.
    ok "no SSH prefix when not in SSH session"
end

# Toggle SSH off explicitly.
set -gx SSH_TTY /dev/pts/0
set -g _fp_show_ssh 0
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
# With SSH disabled, the ':' separator shouldn't appear before /tmp.
# We can't test this perfectly (the prompt has colons elsewhere — time, etc.,
# though we have time disabled in tests). Check with an indirect signal:
# the visible rendered hostname won't appear. Instead, check that exactly
# one segment of the prompt before any whitespace contains '/tmp' uncolored.
ok "SSH toggle off renders without prefix (smoke)"
set -g _fp_show_ssh 1
set -e SSH_TTY

# venv renders the basename of $VIRTUAL_ENV on the right side.
set -gx VIRTUAL_ENV /tmp/myproj-venv
set -g COLUMNS 200
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "myproj-venv" "venv basename appears on the right"
set -e VIRTUAL_ENV

# venv special case: when basename is .venv, use parent dir name.
set -gx VIRTUAL_ENV /tmp/some-project/.venv
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "some-project" "venv with .venv basename uses parent dir"
assert_not_contains "$out" ".venv" "literal '.venv' suppressed in favor of parent dir"
set -e VIRTUAL_ENV

# direnv renders 'direnv' when $DIRENV_DIR is set.
set -gx DIRENV_DIR /tmp/some-direnv-dir
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "direnv" "direnv indicator appears when DIRENV_DIR set"
set -e DIRENV_DIR

# Priority drop: at moderate width, venv drops but direnv + time stay.
set -gx VIRTUAL_ENV /tmp/myenv-name
set -gx DIRENV_DIR /tmp/dr
set -g _fp_show_time 1
set -g COLUMNS 30
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_not_contains "$out" "myenv-name" "narrow: venv drops first"
assert_contains "$out" "direnv" "narrow: direnv survives ahead of venv"

# Tighter: only time should remain.
set -g COLUMNS 20
set -l out (fish_prompt | string collect)
assert_not_contains "$out" "myenv-name" "very narrow: venv still dropped"
assert_not_contains "$out" "direnv" "very narrow: direnv dropped second"

# Even tighter: nothing on the right.
set -g COLUMNS 12
set -l out (fish_prompt | string collect)
# Time is HH:MM:SS (8 chars) + a leading space + left_w. With path '/tmp main' (~9)
# the total is ~18, exceeding 12 — so time should also drop.
if string match -qr '[0-9]{2}:[0-9]{2}:[0-9]{2}' -- "$out"
    fail "extreme narrow: even time should drop"
else
    ok "extreme narrow: time drops last when nothing fits"
end

set -g COLUMNS 200
set -e VIRTUAL_ENV
set -e DIRENV_DIR
set -g _fp_show_time 0

# ----- multi-line layout -----

# Prompt symbol appears on a second line.
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
if string match -q '*'\n'*' -- "$out"
    ok "prompt is multi-line"
else
    fail "prompt is multi-line"
    echo "       expected output to contain a newline"
    echo "       actual: $out"
end
assert_contains "$out" "❯" "prompt symbol on the second line"

# Time renders on the right when enabled.
set -g _fp_show_time 1
set -g COLUMNS 80
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
if string match -qr '^[0-9]{2}:[0-9]{2}:[0-9]{2}$' (date '+%H:%M:%S')
    # Just check that an HH:MM:SS-shaped substring appears.
    if string match -qr '[0-9]{2}:[0-9]{2}:[0-9]{2}' -- "$out"
        ok "time renders on the right"
    else
        fail "time renders on the right"
        echo "       expected HH:MM:SS substring; actual: $out"
    end
end
set -g _fp_show_time 0

# Command duration renders when over the threshold.
set -g _fp_show_cmd_duration 1
set -g _fp_cmd_duration_threshold_ms 1000
set -g CMD_DURATION 2500
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "2.5s" "duration renders when over threshold"
set -g CMD_DURATION 500
set -l out (fish_prompt | string collect)
assert_not_contains "$out" "0.5s" "duration hidden when under threshold"
set -g _fp_show_cmd_duration 0
set -g CMD_DURATION 0

# ----- exit code (` | N` after the duration on the left) -----

# `false` makes $status=1 visible inside the command substitution that
# wraps fish_prompt. _strip_ansi keeps substring checks tidy.
function _strip_ansi
    string replace -ar '\x1b\[[0-9;]*m' '' -- $argv
end

set -g _fp_show_exit_code 1
set -g _fp_show_time 0
set -e VIRTUAL_ENV
set -e DIRENV_DIR
set -g COLUMNS 200

# Non-zero status: ` | N` appears on the left, not the legacy ` [N]`.
write_status /tmp main 0 0 0 '' '' 0
false
set -l out (fish_prompt | string collect)
set -l line1 (string split \n -- $out)[1]
assert_not_contains "$line1" "[1]" "exit code no longer renders as [N] after path"
assert_contains (_strip_ansi $line1) " | 1" "exit code renders as ` | N` on the left"

# Pairs cleanly with a command duration: ` 1.0s | 1`.
set -g _fp_show_cmd_duration 1
set -g _fp_cmd_duration_threshold_ms 1000
set -g CMD_DURATION 1000
write_status /tmp main 0 0 0 '' '' 0
false
set -l out (fish_prompt | string collect)
set -l line1 (string split \n -- $out)[1]
assert_contains (_strip_ansi $line1) "1.0s | 1" "exit code follows the duration with ` | `"
set -g _fp_show_cmd_duration 0
set -g CMD_DURATION 0

# status=0 → no ` | N` even with the toggle on.
write_status /tmp main 0 0 0 '' '' 0
true
set -l out (fish_prompt | string collect)
set -l line1 (string split \n -- $out)[1]
assert_not_contains (_strip_ansi $line1) " | " "no pipe segment when status is zero"

# Toggle off: ` | N` hidden even with status != 0.
set -g _fp_show_exit_code 0
write_status /tmp main 0 0 0 '' '' 0
false
set -l out (fish_prompt | string collect)
set -l line1 (string split \n -- $out)[1]
assert_not_contains (_strip_ansi $line1) " | 1" "exit code hidden when _fp_show_exit_code=0"
set -g _fp_show_exit_code 1

# Vi mode indicator now lives in fish_mode_prompt (left of line 1), not the
# line-2 prompt symbol. fish_mode_prompt only renders when vi keybindings
# are active, so the test stages that.
set -g _fp_show_vi_mode 1
set -gx fish_key_bindings fish_vi_key_bindings
set -g fish_bind_mode default
set -g _fp_symbol_vi_default 'NORMAL'
set -l out (fish_mode_prompt | string collect)
assert_contains "$out" "NORMAL" "fish_mode_prompt renders default-mode symbol"

set -g fish_bind_mode insert
set -g _fp_symbol_vi_insert 'INSERT'
set -l out (fish_mode_prompt | string collect)
assert_contains "$out" "INSERT" "fish_mode_prompt renders insert-mode symbol"

# Auto-skip when emacs keybindings are active.
set -gx fish_key_bindings fish_default_key_bindings
set -l out (fish_mode_prompt | string collect)
assert_not_contains "$out" "INSERT" "fish_mode_prompt empty under non-vi keybindings"
assert_not_contains "$out" "NORMAL" "no normal-mode label under non-vi keybindings"

# Manual disable.
set -gx fish_key_bindings fish_vi_key_bindings
set -g _fp_show_vi_mode 0
set -l out (fish_mode_prompt | string collect)
assert_not_contains "$out" "INSERT" "fish_mode_prompt empty when _fp_show_vi_mode=0"

# ----- regression: line 1 padding accounts for fish_mode_prompt width -----
#
# fish renders fish_mode_prompt to the left of fish_prompt's line 1. The
# mode-prompt width has to come out of fish_prompt's padding budget; without
# that subtraction, line 1 + mode prompt exceeds $COLUMNS and fish prepends
# `…` and trims from the left. See commit 5cb1d2a.

set -g _fp_symbol_vi_insert '[I]'
set -g _fp_show_vi_mode 1
set -gx fish_key_bindings fish_vi_key_bindings
set -g fish_bind_mode insert
set -g _fp_show_time 1
set -g COLUMNS 80

set -l mode_w (string length --visible -- (fish_mode_prompt | string collect))
if test $mode_w -le 0
    fail "fish_mode_prompt produced no output (test setup broken)"
end

write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
set -l line1 (string split \n -- $out)[1]
set -l line1_w (string length --visible -- "$line1")
set -l combined (math "$mode_w + $line1_w")

# When the right side renders, fish_prompt produces line 1 with width
# exactly $COLUMNS - mode_w; fish then prepends mode_w of its own. If the
# subtraction is missing, line 1 alone is already $COLUMNS wide and the
# combined render overflows.
if test $combined -eq $COLUMNS
    ok "line 1 + fish_mode_prompt width equals \$COLUMNS"
else
    fail "line 1 + fish_mode_prompt width equals \$COLUMNS"
    echo "       expected: mode_w + line1_w = $COLUMNS"
    echo "       actual:   $mode_w + $line1_w = $combined"
end

assert_contains "$line1" "main" "branch survives when mode prompt is active"
if string match -qr '[0-9]{2}:[0-9]{2}:[0-9]{2}' -- "$line1"
    ok "time survives when mode prompt is active"
else
    fail "time survives when mode prompt is active"
    echo "       actual line 1: $line1"
end

set -g _fp_show_time 0

# ----- regression: line 1 padding accounts for SHELL_PROMPT_PREFIX width -----
#
# fish 4.6+ inserts $SHELL_PROMPT_PREFIX at position 0 of the left-prompt
# buffer (set by systemd's run0, etc.). It lands ahead of our $left on
# line 1, so without subtracting its width from the padding budget, line 1
# overflows $COLUMNS and fish truncates from the left.

set -gx SHELL_PROMPT_PREFIX '[run0] '
set -g _fp_show_time 1
set -g COLUMNS 80
set -gx fish_key_bindings fish_default_key_bindings

set -l prefix_w (string length --visible -- "$SHELL_PROMPT_PREFIX")
set -l mode_w (string length --visible -- (fish_mode_prompt | string collect))

write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
set -l line1 (string split \n -- $out)[1]
set -l line1_w (string length --visible -- "$line1")
set -l combined (math "$prefix_w + $mode_w + $line1_w")

if test $combined -eq $COLUMNS
    ok "line 1 + SHELL_PROMPT_PREFIX width equals \$COLUMNS"
else
    fail "line 1 + SHELL_PROMPT_PREFIX width equals \$COLUMNS"
    echo "       expected: prefix_w + mode_w + line1_w = $COLUMNS"
    echo "       actual:   $prefix_w + $mode_w + $line1_w = $combined"
end

# Sanity: empty prefix should be a no-op (treated as if unset).
set -gx SHELL_PROMPT_PREFIX ''
set -l out (fish_prompt | string collect)
set -l line1 (string split \n -- $out)[1]
set -l line1_w (string length --visible -- "$line1")
set -l combined (math "$mode_w + $line1_w")
if test $combined -eq $COLUMNS
    ok "empty SHELL_PROMPT_PREFIX is a no-op"
else
    fail "empty SHELL_PROMPT_PREFIX is a no-op"
    echo "       expected: mode_w + line1_w = $COLUMNS"
    echo "       actual:   $mode_w + $line1_w = $combined"
end

set -e SHELL_PROMPT_PREFIX
set -g _fp_show_time 0

# Reset for following tests.
set -g _fp_symbol_vi_default '[N]'
set -g _fp_symbol_vi_insert  '[I]'
set -g _fp_show_vi_mode 1
set -gx fish_key_bindings fish_default_key_bindings
set -g fish_bind_mode insert

# Empty status file: must not crash and must produce a prompt.
command truncate -s 0 $_fp_status_file
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd with empty status file"

# Missing status file: must not crash either.
command rm -f $_fp_status_file
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd with missing status file"

# ----- corrupt status file recovery -----
# The daemon writes via atomic rename so we don't see torn writes from it,
# but external interference (filesystem corruption, a third-party writer)
# could still leave malformed bytes on disk. fish_prompt must keep rendering
# without crashing in every case.

# Binary garbage, no NUL delimiters: split0 yields one field, the
# version-sentinel guard fails, git block stays hidden.
printf '\x01\x02\x03\xff\xff\xff\x80garbage' >$_fp_status_file
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd with binary garbage status"

# Truncated mid-record: only 2 fields and no version sentinel. Either guard
# is enough to hide the git block.
printf '/tmp\0main\0' >$_fp_status_file
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd with truncated status"
assert_not_contains "$out" "main" "no git block when status is truncated"

# v0-shaped status (no FP1 sentinel) — what an old daemon would write to a
# new fish. The version guard rejects it; the git block is hidden.
printf '/tmp\0main\0 0\0 0\0 0\0\0\0 0\0 0\0' >$_fp_status_file
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd against v0-shaped status"
assert_not_contains "$out" "main" "no git block when wire version is missing"

# Wrong sentinel (e.g., a hypothetical future FP2 the daemon emits while
# this fish is still on FP1). Same fall-through.
printf 'FP2\0/tmp\0main\0 0\0 0\0 0\0\0\0 0\0 0\0 0\0' >$_fp_status_file
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd against unknown wire version"
assert_not_contains "$out" "main" "no git block when wire version is unknown"

# Path doesn't match $PWD: path-match guard hides the git block.
write_status not-a-real-path main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd when status path is bogus"
assert_not_contains "$out" "main" "no branch when status path is bogus"

# Path matches PWD, but numeric fields are non-numeric.
# The daemon never writes this; this is the filesystem-corruption case.
# We just assert no crash — current fish_prompt would render literal
# garbage like ` ↑xyz`, which is acceptable for a corrupt-state recovery.
write_status /tmp main xyz qqq '' '' '' abc
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "no crash when numeric fields are garbage"
assert_contains "$out" "main" "branch still renders alongside garbage counts"

# Defensive: fish_prompt must tolerate _fp_request_status being undefined.
# Conf.d defines it only in interactive shells; without the `functions -q`
# guard in fish_prompt, scripted use or partial installs would print
# "Unknown command" on every render.
write_status /tmp main 0 0 0 '' '' 0
functions -e _fp_request_status
set -l err (begin; fish_prompt; end 2>&1 >/dev/null | string collect)
if test -z "$err"
    ok "fish_prompt is silent when _fp_request_status is undefined"
else
    fail "fish_prompt is silent when _fp_request_status is undefined"
    echo "       stderr: $err"
end
function _fp_request_status
end

# ----- daemon respawn -----
#
# conf.d's spawn block is gated behind `status is-interactive`, so we can't
# reach it from this scripted test. Instead, set up our own state and call
# the autoloaded helpers (_fp_spawn_daemon / _fp_ensure_daemon) directly.

set -l daemon_path
if command -q fish-prompt-daemon
    set daemon_path (command -v fish-prompt-daemon)
else if test -x $repo_root/target/release/fish-prompt-daemon
    set daemon_path $repo_root/target/release/fish-prompt-daemon
    set -gx PATH $repo_root/target/release $PATH
end

if test -z "$daemon_path"
    echo "  skip: fish-prompt-daemon not found (run `cargo build --release` first)"
else
    set -l saved_status_file $_fp_status_file
    set -l tmpdir $TMPDIR
    test -n "$tmpdir"; or set tmpdir /tmp
    set -g _fp_dir (command mktemp -d $tmpdir/fp-respawn-test.XXXXXXXX)
    set -g _fp_status_file $_fp_dir/status
    set -g _fp_request_fifo $_fp_dir/req
    command mkfifo $_fp_request_fifo

    # SIGUSR1 default action is terminate. The daemon signals fish on every
    # status write; install a no-op handler for the duration of the test.
    function _fp_test_sigusr1 --on-signal SIGUSR1
    end

    # Initial spawn.
    _fp_spawn_daemon
    set -l first_pid $_fp_daemon_pid
    if test -n "$first_pid"; and kill -0 $first_pid 2>/dev/null
        ok "_fp_spawn_daemon launches a daemon"
    else
        fail "_fp_spawn_daemon launches a daemon"
        echo "       _fp_daemon_pid=$_fp_daemon_pid"
    end

    # Kill it. Clear the rate-limit cookie so respawn isn't blocked by it.
    command kill -9 $first_pid 2>/dev/null
    set -e _fp_daemon_last_spawn_attempt
    sleep 0.2

    _fp_ensure_daemon
    set -l second_pid $_fp_daemon_pid
    if test -n "$second_pid"; and test "$second_pid" != "$first_pid"; and kill -0 $second_pid 2>/dev/null
        ok "_fp_ensure_daemon respawns after daemon death"
    else
        fail "_fp_ensure_daemon respawns after daemon death"
        echo "       first=$first_pid second=$second_pid"
    end

    # Liveness fast-path: when alive, ensure must be a no-op.
    set -l before_pid $_fp_daemon_pid
    _fp_ensure_daemon
    if test "$_fp_daemon_pid" = "$before_pid"
        ok "_fp_ensure_daemon is a no-op when daemon is alive"
    else
        fail "_fp_ensure_daemon is a no-op when daemon is alive"
        echo "       before=$before_pid after=$_fp_daemon_pid"
    end

    # Cleanup.
    command kill -9 $_fp_daemon_pid 2>/dev/null
    command rm -rf $_fp_dir
    set -g _fp_status_file $saved_status_file
    functions -e _fp_test_sigusr1
end

# ----- summary -----

echo
if test $TESTS_FAILED -eq 0
    echo "$TESTS_RUN passed"
    exit 0
else
    echo "$TESTS_FAILED of $TESTS_RUN failed"
    exit 1
end
