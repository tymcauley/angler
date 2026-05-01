#!/usr/bin/env fish
# Tests for fish_prompt rendering. Drives the function with hand-written
# status files; does not exercise the daemon or signal flow.

set -l repo_root (realpath (dirname (status filename))/..)
set -ga fish_function_path $repo_root/functions

# Source conf.d to pick up the symbol/color/toggle defaults. The interactive
# gate inside it short-circuits before the daemon-spawn block, so this is safe
# in scripted mode.
source $repo_root/conf.d/fish-prompt.fish

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

function write_status -d "Write an 8-field NUL-delimited status file: path branch ahead behind dirty operation upstream stash"
    printf '%s\0%s\0%s\0%s\0%s\0%s\0%s\0%s\0' $argv >$_fp_status_file
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

# Vi mode: 'default' (vim normal) uses the vi-default symbol when enabled.
set -g _fp_show_vi_mode 1
set -g fish_bind_mode default
set -g _fp_symbol_vi_default 'NORMAL'
write_status /tmp main 0 0 0 '' '' 0
set -l out (fish_prompt | string collect)
assert_contains "$out" "NORMAL" "vi default mode shows custom symbol"
set -g _fp_symbol_vi_default '❮'
set -g _fp_show_vi_mode 0
set -g fish_bind_mode insert

# Empty status file: must not crash and must produce a prompt.
command truncate -s 0 $_fp_status_file
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd with empty status file"

# Missing status file: must not crash either.
command rm -f $_fp_status_file
set -l out (fish_prompt | string collect)
assert_contains "$out" "tmp" "still renders cwd with missing status file"

# ----- summary -----

echo
if test $TESTS_FAILED -eq 0
    echo "$TESTS_RUN passed"
    exit 0
else
    echo "$TESTS_FAILED of $TESTS_RUN failed"
    exit 1
end
