function fish_prompt
    set -l last_status $status

    set_color $_fp_color_path
    echo -n (prompt_pwd)
    set_color normal

    if test $last_status -ne 0; and test "$_fp_show_exit_code" = 1
        set_color $_fp_color_exit_code
        echo -n " [$last_status]"
        set_color normal
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

            # Render only when the response matches the current PWD; otherwise
            # the daemon hasn't caught up yet and any data we have is stale.
            if test "$reported_path" = $PWD; and test -n "$branch"
                set_color $_fp_color_branch
                echo -n " $branch"
                if test -n "$operation"; and test "$_fp_show_operation" = 1
                    set_color $_fp_color_operation
                    echo -n " ($operation)"
                end
                if test "$_fp_show_ahead_behind" = 1
                    if test "$ahead" != 0; and test -n "$ahead"
                        set_color $_fp_color_ahead
                        echo -n "$_fp_symbol_ahead$ahead"
                    end
                    if test "$behind" != 0; and test -n "$behind"
                        set_color $_fp_color_behind
                        echo -n "$_fp_symbol_behind$behind"
                    end
                end
                if test "$upstream" = gone
                    set_color $_fp_color_gone
                    echo -n " $_fp_symbol_gone"
                end
                # Dirty wire encoding: "0" clean, "?" unknown, otherwise any
                # combination of '+' staged, '*' modified, 'u' untracked,
                # '!' conflict.
                if test "$dirty" = '?'
                    set_color $_fp_color_unknown
                    echo -n " $_fp_symbol_unknown"
                else if test -n "$dirty"; and test "$dirty" != 0
                    for c in (string split '' -- $dirty)
                        # Fish case patterns are globs — '*' would match
                        # anything, so escape it.
                        switch $c
                            case '+'
                                set_color $_fp_color_staged
                                echo -n " $_fp_symbol_staged"
                            case '\*'
                                set_color $_fp_color_modified
                                echo -n " $_fp_symbol_modified"
                            case u
                                set_color $_fp_color_untracked
                                echo -n " $_fp_symbol_untracked"
                            case '!'
                                set_color $_fp_color_conflict
                                echo -n " $_fp_symbol_conflict"
                        end
                    end
                end
                if test "$stash" != 0; and test -n "$stash"; and test "$_fp_show_stash" = 1
                    set_color $_fp_color_stash
                    echo -n " $_fp_symbol_stash$stash"
                end
                set_color normal
            end
        end
    end

    echo
    echo -n "$_fp_symbol_prompt "
end
