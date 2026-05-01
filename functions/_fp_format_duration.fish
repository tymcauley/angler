function _fp_format_duration --argument-names ms
    # Returns a short human-readable duration string. Empty for very short
    # durations (caller is expected to gate on threshold separately if it
    # cares); otherwise:
    #   1ms..999ms   -> 'Xms'
    #   1s..59s      -> 'X.Ys'
    #   1m..59m59s   -> 'XmYs'
    #   1h+          -> 'XhYm'
    if test -z "$ms"; or test "$ms" -le 0
        return
    end
    if test $ms -lt 1000
        printf '%dms' $ms
    else if test $ms -lt 60000
        printf '%.1fs' (math "$ms / 1000")
    else if test $ms -lt 3600000
        set -l m (math "floor($ms / 60000)")
        set -l s (math "floor(($ms % 60000) / 1000)")
        printf '%dm%ds' $m $s
    else
        set -l h (math "floor($ms / 3600000)")
        set -l m (math "floor(($ms % 3600000) / 60000)")
        printf '%dh%dm' $h $m
    end
end
