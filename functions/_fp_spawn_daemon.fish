function _fp_spawn_daemon
    # Spawns the daemon and stashes its PID into $_fp_daemon_pid so
    # _fp_ensure_daemon can probe liveness with `kill -0` later.
    # Relies on globals set up by conf.d: $_fp_status_file, $_fp_request_fifo,
    # $_fp_dirty_deadline_ms, plus fish's $fish_pid. $_fp_log_file is
    # optional — when empty, no --log-file is passed and the daemon runs
    # without logging.
    set -l args \
        --fish-pid $fish_pid \
        --status-file $_fp_status_file \
        --request-fifo $_fp_request_fifo \
        --dirty-deadline-ms $_fp_dirty_deadline_ms
    if test -n "$_fp_log_file"
        set args $args --log-file $_fp_log_file
    end
    command fish-prompt-daemon $args &
    set -g _fp_daemon_pid $last_pid
    disown $_fp_daemon_pid 2>/dev/null
end
