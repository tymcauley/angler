function _angler_spawn_daemon
    # Spawns the daemon and stashes its PID into $_angler_daemon_pid so
    # _angler_ensure_daemon can probe liveness with `kill -0` later.
    # Relies on globals set up by _angler_init: $_angler_dir,
    # $_angler_dirty_deadline_ms, plus fish's $fish_pid. $_angler_log_file is
    # optional — when empty, no --log-file is passed and the daemon runs
    # without logging.
    set -l args \
        --fish-pid $fish_pid \
        --state-dir $_angler_dir \
        --dirty-deadline-ms $_angler_dirty_deadline_ms
    if test -n "$_angler_log_file"
        set args $args --log-file $_angler_log_file
    end
    command angler-daemon $args &
    set -g _angler_daemon_pid $last_pid
    # Write the pidfile synchronously here (rather than from the daemon)
    # so adoption across `exec fish` can't race the daemon's startup. The
    # pidfile path is set up by _angler_init alongside _angler_dir.
    set -q _angler_pid_file; and echo $_angler_daemon_pid >$_angler_pid_file
    disown $_angler_daemon_pid 2>/dev/null
end
