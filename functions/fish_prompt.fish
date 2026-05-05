function fish_prompt
    set -l last_status $status
    set -l cmd_duration $CMD_DURATION

    # Kick the daemon to recompute on every prompt render. The daemon
    # responds asynchronously and SIGUSR1's a repaint when the result
    # actually changes — see the idempotency guard in the daemon. This
    # keeps the prompt correct after external worktree changes (editor
    # saves, scripts in another window, etc.) that don't touch `.git/`.
    # Guarded so this file is safe to source outside our conf.d (tests,
    # partial installs); conf.d defines the function in interactive shells.
    if functions -q _fp_request_status
        _fp_request_status
    end

    # fish renders fish_mode_prompt to the left of fish_prompt's first line;
    # its width has to come out of our padding budget, otherwise an indicator
    # like `[I] ` pushes line 1 past $COLUMNS and fish truncates from the
    # left (prepending `…`).
    set -l mode_str (fish_mode_prompt | string collect)
    set -l mode_w 0
    if test -n "$mode_str"
        set mode_w (string length --visible -- "$mode_str")
    end

    # Build line 1 from left and right halves. Left always renders fully;
    # right side has its own priority-drop logic (in _fp_render_right) so
    # that low-priority indicators yield to higher-priority ones when the
    # terminal is narrow.
    set -l left (_fp_render_left $last_status $cmd_duration | string collect)
    set -l left_w (string length --visible -- "$left")

    set -l effective_left_w (math "$left_w + $mode_w")
    set -l right (_fp_render_right $effective_left_w | string collect)
    set -l right_w (string length --visible -- "$right")

    set -l pad (math "$COLUMNS - $effective_left_w - $right_w")
    if test -n "$right"; and test $pad -ge 1
        printf '%s%*s%s\n' "$left" $pad "" "$right"
    else
        printf '%s\n' "$left"
    end

    _fp_render_prompt_symbol
end

# Each `set buf $buf X Y Z` appends X, Y, Z as separate list elements; we then
# `string join ''` at the end. Avoids fish's cartesian-product behavior on
# `$buf(cmd)` which produces nothing when $buf is the empty list.
function _fp_render_left --argument-names last_status cmd_duration
    set -l buf

    # SSH host prefix — `host:` before the path when SSH'd in. Tells the
    # user the path that follows is on a remote machine.
    if test "$_fp_show_ssh" = 1; and set -q SSH_TTY
        set buf $buf (set_color $_fp_color_ssh) (prompt_hostname)":" (set_color normal)
    end

    # Render the path with the last component emphasized. `prompt_pwd` returns
    # something like `~/c/fish-prompt`; we color the prefix (`~/c/`) with
    # `_fp_color_path` and the tail (`fish-prompt`) with `_fp_color_path_tail`.
    set -l pwd_str (prompt_pwd)
    set -l parts (string split / -- $pwd_str)
    set -l tail $parts[-1]
    if test (count $parts) -gt 1
        set -l prefix (string join / -- $parts[1..-2])/
        set buf $buf (set_color $_fp_color_path) $prefix (set_color $_fp_color_path_tail) $tail (set_color normal)
    else
        set buf $buf (set_color $_fp_color_path_tail) $tail (set_color normal)
    end

    if set -q _fp_status_file; and test -r $_fp_status_file
        set -l fields (cat $_fp_status_file | string split0)
        if test (count $fields) -ge 8
            set -l reported_path $fields[1]
            set -l branch $fields[2]
            set -l ahead $fields[3]
            set -l behind $fields[4]
            set -l dirty $fields[5]
            set -l operation $fields[6]
            set -l upstream $fields[7]
            set -l stash $fields[8]

            if test "$reported_path" = $PWD; and test -n "$branch"
                set buf $buf (set_color $_fp_color_branch) " $branch"
                if test -n "$operation"; and test "$_fp_show_operation" = 1
                    set buf $buf (set_color $_fp_color_operation) " ($operation)"
                end
                if test "$_fp_show_ahead_behind" = 1
                    if test "$ahead" != 0; and test -n "$ahead"
                        set buf $buf (set_color $_fp_color_ahead) " $_fp_symbol_ahead$ahead"
                    end
                    if test "$behind" != 0; and test -n "$behind"
                        set buf $buf (set_color $_fp_color_behind) " $_fp_symbol_behind$behind"
                    end
                end
                if test "$upstream" = gone
                    set buf $buf (set_color $_fp_color_gone) " $_fp_symbol_gone"
                end
                if test "$dirty" = '?'
                    set buf $buf (set_color $_fp_color_unknown) " $_fp_symbol_unknown"
                else if test -n "$dirty"; and test "$dirty" != 0
                    for c in (string split '' -- $dirty)
                        switch $c
                            case '+'
                                set buf $buf (set_color $_fp_color_staged) " $_fp_symbol_staged"
                            case '\*'
                                set buf $buf (set_color $_fp_color_modified) " $_fp_symbol_modified"
                            case u
                                set buf $buf (set_color $_fp_color_untracked) " $_fp_symbol_untracked"
                            case '!'
                                set buf $buf (set_color $_fp_color_conflict) " $_fp_symbol_conflict"
                        end
                    end
                end
                if test "$stash" != 0; and test -n "$stash"; and test "$_fp_show_stash" = 1
                    set buf $buf (set_color $_fp_color_stash) " $_fp_symbol_stash$stash"
                end
                set buf $buf (set_color normal)
            end
        end
    end

    # Command duration on the left, after git. Always shown when over
    # threshold, regardless of how narrow the terminal is.
    if test "$_fp_show_cmd_duration" = 1; and test -n "$cmd_duration"; \
        and test "$cmd_duration" -ge "$_fp_cmd_duration_threshold_ms"
        set -l dur (_fp_format_duration $cmd_duration)
        if test -n "$dur"
            set buf $buf (set_color $_fp_color_duration) " $dur" (set_color normal)
        end
    end

    # Exit code, after duration. ` | N` where the pipe and the code share
    # `_fp_color_exit_code` so they read as one unit.
    if test "$last_status" -ne 0; and test "$_fp_show_exit_code" = 1
        set buf $buf (set_color $_fp_color_exit_code) " | $last_status" (set_color normal)
    end

    string join '' -- $buf
end

# Builds the right-side content (venv, direnv, time) and drops segments in
# priority order if the combined width doesn't fit alongside the left.
# Drop order matches the order they're added below (leftmost first).
function _fp_render_right --argument-names left_w
    set -l segs
    set -l prios

    if test "$_fp_show_venv" = 1; and set -q VIRTUAL_ENV
        set -l name (basename $VIRTUAL_ENV)
        # Common pattern: project/.venv → use parent dir name (more useful
        # than the literal ".venv").
        if test "$name" = .venv; or test "$name" = venv
            set name (basename (dirname $VIRTUAL_ENV))
        end
        set segs $segs (printf '%s%s%s' (set_color $_fp_color_venv) $name (set_color normal))
        set prios $prios venv
    end

    if test "$_fp_show_direnv" = 1; and set -q DIRENV_DIR
        set segs $segs (printf '%s%s%s' (set_color $_fp_color_direnv) direnv (set_color normal))
        set prios $prios direnv
    end

    if test "$_fp_show_time" = 1
        set segs $segs (printf '%s%s%s' (set_color $_fp_color_time) (date '+%H:%M:%S') (set_color normal))
        set prios $prios time
    end

    set -l drop_order venv direnv time

    while test (count $segs) -gt 0
        set -l combined (string join ' ' -- $segs)
        set -l combined_w (string length --visible -- "$combined")
        set -l pad (math "$COLUMNS - $left_w - $combined_w")
        if test $pad -ge 1
            echo $combined
            return
        end
        # Doesn't fit; drop the highest-priority-to-drop segment.
        set -l dropped 0
        for d in $drop_order
            set -l idx (contains -i -- $d $prios)
            if test -n "$idx"
                set -e segs[$idx]
                set -e prios[$idx]
                set dropped 1
                break
            end
        end
        # Defensive: if nothing matched drop_order (shouldn't happen since
        # all priorities we emit are listed), bail rather than loop forever.
        if test $dropped -eq 0
            break
        end
    end
end

function _fp_render_prompt_symbol
    set_color $_fp_color_prompt
    printf '%s ' $_fp_symbol_prompt
    set_color normal
end
