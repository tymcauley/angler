function _angler_spawn_daemon
    # Spawns the daemon and stashes its PID into $_angler_daemon_pid so
    # _angler_ensure_daemon can probe liveness with `kill -0` later.
    # Relies on globals set up by conf.d: $_angler_status_file, $_angler_request_fifo,
    # $_angler_dirty_deadline_ms, plus fish's $fish_pid. $_angler_log_file is
    # optional — when empty, no --log-file is passed and the daemon runs
    # without logging.
    set -l args \
        --fish-pid $fish_pid \
        --status-file $_angler_status_file \
        --request-fifo $_angler_request_fifo \
        --dirty-deadline-ms $_angler_dirty_deadline_ms
    if test -n "$_angler_log_file"
        set args $args --log-file $_angler_log_file
    end
    command angler-daemon $args &
    set -g _angler_daemon_pid $last_pid
    disown $_angler_daemon_pid 2>/dev/null
end
