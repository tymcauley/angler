function fish_prompt
    set -l last_status $status
    set -l cmd_duration $CMD_DURATION

    # Build line 1's left and right halves as strings with embedded ANSI
    # codes. We measure visible widths via `string length --visible` so
    # padding ignores escape sequences.
    set -l left_buf (_fp_render_left $last_status | string collect)
    set -l right_buf (_fp_render_right $cmd_duration | string collect)

    set -l left_w (string length --visible -- "$left_buf")
    set -l right_w (string length --visible -- "$right_buf")
    set -l pad (math "$COLUMNS - $left_w - $right_w")
    if test -n "$right_buf"; and test $pad -ge 1
        printf '%s%*s%s\n' "$left_buf" $pad "" "$right_buf"
    else
        printf '%s\n' "$left_buf"
    end

    _fp_render_prompt_symbol
end

# Each `set buf $buf X Y Z` appends X, Y, Z as separate list elements; we then
# `string join ''` at the end. Avoids fish's cartesian-product behavior on
# `$buf(cmd)` which produces nothing when $buf is the empty list.
function _fp_render_left --argument-names last_status
    set -l buf
    set buf $buf (set_color $_fp_color_path) (prompt_pwd) (set_color normal)

    if test "$last_status" -ne 0; and test "$_fp_show_exit_code" = 1
        set buf $buf (set_color $_fp_color_exit_code) " [$last_status]" (set_color normal)
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
                        set buf $buf (set_color $_fp_color_ahead) "$_fp_symbol_ahead$ahead"
                    end
                    if test "$behind" != 0; and test -n "$behind"
                        set buf $buf (set_color $_fp_color_behind) "$_fp_symbol_behind$behind"
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

    string join '' -- $buf
end

function _fp_render_right --argument-names cmd_duration
    set -l buf

    if test "$_fp_show_cmd_duration" = 1; and test -n "$cmd_duration"; \
        and test "$cmd_duration" -ge "$_fp_cmd_duration_threshold_ms"
        set -l dur (_fp_format_duration $cmd_duration)
        if test -n "$dur"
            set buf $buf (set_color $_fp_color_duration) "$dur" (set_color normal)
        end
    end

    if test "$_fp_show_time" = 1
        set -l now (date '+%H:%M:%S')
        if test (count $buf) -gt 0
            set buf $buf ' '
        end
        set buf $buf (set_color $_fp_color_time) "$now" (set_color normal)
    end

    string join '' -- $buf
end

function _fp_render_prompt_symbol
    if test "$_fp_show_vi_mode" = 1
        switch $fish_bind_mode
            case default
                set_color $_fp_color_vi_default
                printf '%s ' $_fp_symbol_vi_default
                set_color normal
                return
            case visual
                set_color $_fp_color_vi_visual
                printf '%s ' $_fp_symbol_vi_visual
                set_color normal
                return
            case replace replace_one
                set_color $_fp_color_vi_replace
                printf '%s ' $_fp_symbol_vi_replace
                set_color normal
                return
        end
    end
    printf '%s ' $_fp_symbol_prompt
end
