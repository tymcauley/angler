# Fired by fisher's plugin event system on install AND update of
# tymcauley/angler. The fish files alone aren't enough — angler needs the
# `angler-daemon` binary on PATH. We don't auto-download (the fish-plugin
# convention is to leave binary management to the user), but we DO check
# whether the right version is installed and print a clear hint if not.
#
# `$expected` is bumped in lockstep with each plugin release that requires
# a new daemon. The on-the-wire version sentinel handles partial installs
# gracefully (no git block instead of misparsed status), so the worst
# outcome of ignoring this hint is a degraded prompt — never a crash.
function _angler_install --on-event angler_install --on-event angler_update
    set -l expected 0.1.0

    set -l install_help \
        "  cargo install --locked --git https://github.com/tymcauley/angler --tag v$expected" \
        "  (or download a prebuilt tarball: https://github.com/tymcauley/angler/releases/tag/v$expected)" \
        "Then run \`exec fish\`."

    if not command -q angler-daemon
        echo "angler: angler-daemon binary not found on PATH."
        echo "Install it with one of:"
        printf '%s\n' $install_help
        return 0
    end

    set -l found (command angler-daemon --version 2>/dev/null \
                  | string match -r 'angler-daemon (\S+)' --groups-only)
    if test -z "$found"
        echo "angler: angler-daemon found on PATH but doesn't report a version."
        echo "Reinstall with:"
        printf '%s\n' $install_help
        return 0
    end

    if test "$found" != "$expected"
        echo "angler: angler-daemon version mismatch."
        echo "  installed: $found"
        echo "  expected:  $expected"
        echo "Upgrade with:"
        printf '%s\n' $install_help
        return 0
    end
end
