function _fp_ensure_daemon
    # Fast path: daemon is alive, nothing to do.
    if test -n "$_fp_daemon_pid"; and kill -0 $_fp_daemon_pid 2>/dev/null
        return
    end

    # Rate-limit respawns. If the daemon binary panics on startup we'd
    # otherwise fork it on every cd; better to leave the prompt without
    # git info for a second than to fork-bomb the system.
    set -l now (date +%s)
    if set -q _fp_daemon_last_spawn_attempt; and test (math "$now - $_fp_daemon_last_spawn_attempt") -lt 1
        return
    end
    set -g _fp_daemon_last_spawn_attempt $now

    _fp_spawn_daemon
end
