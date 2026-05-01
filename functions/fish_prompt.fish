function fish_prompt
    set -l last_status $status

    set_color cyan
    echo -n (prompt_pwd)
    set_color normal

    if test $last_status -ne 0
        set_color red
        echo -n " [$last_status]"
        set_color normal
    end

    if set -q _fp_status_file; and test -r $_fp_status_file
        set -l fields (cat $_fp_status_file | string split0)
        if test (count $fields) -ge 5
            set -l reported_path $fields[1]
            set -l branch $fields[2]
            set -l ahead $fields[3]
            set -l behind $fields[4]
            set -l dirty $fields[5]

            # Render only when the response matches the current PWD; otherwise
            # the daemon hasn't caught up yet and any data we have is stale.
            if test "$reported_path" = $PWD; and test -n "$branch"
                set_color yellow
                echo -n " $branch"
                test "$ahead" != 0; and test -n "$ahead"; and echo -n "↑$ahead"
                test "$behind" != 0; and test -n "$behind"; and echo -n "↓$behind"
                switch $dirty
                    case 1
                        set_color red
                        echo -n ' *'
                    case '?'
                        set_color yellow
                        echo -n ' ?'
                end
                set_color normal
            end
        end
    end

    echo
    echo -n '❯ '
end
