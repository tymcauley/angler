function _angler_init
    set -q _angler_init_done; and return
    set -g _angler_init_done 1

    # Daemon binary missing: stay quiet for the rest of this fish session
    # (picking up a later install requires `exec fish` either way).
    command -q angler-daemon; or return

    # Per-PID state dir at a deterministic path. Across `exec fish`, the
    # kernel task at $fish_pid is unchanged, so a daemon spawned by the
    # previous fish image is still ours — adopt it instead of orphaning it
    # and respawning. Falls back through XDG_RUNTIME_DIR → TMPDIR → /tmp;
    # macOS has no XDG_RUNTIME_DIR and lands under /var/folders/.../T/.
    set -l base $XDG_RUNTIME_DIR
    test -n "$base"; or set base $TMPDIR
    test -n "$base"; or set base /tmp

    set -g _angler_dir $base/angler/$fish_pid
    set -g _angler_status_file $_angler_dir/status
    set -g _angler_request_fifo $_angler_dir/req
    set -g _angler_pid_file $_angler_dir/daemon.pid

    # Adoption: an existing FIFO + a pidfile pointing at a live process is
    # a daemon left behind by a previous fish image at this PID.
    if test -p $_angler_request_fifo; and test -r $_angler_pid_file
        read existing <$_angler_pid_file
        if test -n "$existing"; and kill -0 $existing 2>/dev/null
            set -g _angler_daemon_pid $existing
            set -g _angler_init_ok 1
            return
        end
    end

    # Cold start. rm -rf clears any stale state from a previous PID-holder
    # (daemon crash, kill -9 of fish before cleanup ran, etc.).
    command rm -rf $_angler_dir
    command mkdir -p $_angler_dir
    command mkfifo $_angler_request_fifo
    _angler_spawn_daemon
    set -g _angler_init_ok 1
end
