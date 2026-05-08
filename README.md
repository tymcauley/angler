# angler

A fast, asynchronous git prompt for fish 4.
A per-shell Rust daemon computes status with [gix](https://github.com/GitoxideLabs/gitoxide), watches `.git/` via [`notify`](https://github.com/notify-rs/notify), and pokes fish with SIGUSR1 when there's something new to render.
Branch info appears the moment you hit Enter; dirty / ahead / behind fill in shortly after; external git operations (in-editor commits, GUI clients, scripts in another window) update the prompt without you typing anything.

## Output

```
host:~/code/myproject main ↑2 * 1.4s                         myproject direnv 14:23:01
❯
```

Two-line layout: path + git + command duration on line 1 (left), environmental indicators + time on line 1 (right, padded to the terminal edge), prompt symbol on line 2.

Line 1 left:

- **red bold** `host:` prefix when SSH'd in (short hostname, signaling that the path that follows is on a remote machine)
- **cyan** abbreviated working directory: truncated parent components in plain cyan, the directory you're actually in in **bold cyan** (separately configurable as `_angler_color_path` and `_angler_color_path_tail`)
- **yellow** branch name (or 7-char SHA if detached)
- `↑N` commits ahead of upstream, `↓N` behind
- **red** `↯` if the upstream tracking branch is gone (typically a deleted remote branch you can prune)
- Dirty markers: red `*` modified, green `+` staged, yellow `?` untracked, red bold `!` conflict
- **yellow** `?` if the dirty check couldn't finish within the deadline (default 200ms — you'll see it on huge repos with a cold disk cache; resolves on its own once the background scan finishes)
- **blue** `≡N` for stash count (hidden if zero)
- **yellow** `sN` for the count of submodules with changes (hidden if zero — granularity follows your `diff.ignoreSubmodules` config; an unset config defaults to counting both HEAD-diffs and worktree-dirty submodules)
- **magenta** `(rebasing)` / `(merging)` / etc. when an operation is in progress
- **yellow** command duration (only for commands over `_angler_cmd_duration_threshold_ms`, default 1s; always shown when applicable, regardless of terminal width)
- **red bold** ` | N` after the duration if the last command exited non-zero — the `|` shares the exit code's red bold so the two read as one unit. Always shown when applicable, regardless of terminal width.

Line 1 right (only when there's room):

- **blue** Python venv name when `$VIRTUAL_ENV` is set (basename of the venv path; if that's `.venv` or `venv`, walks up to use the parent directory name)
- **green** `direnv` indicator when `$DIRENV_DIR` is set
- **gray** time `HH:MM:SS`

When the terminal is too narrow, the indicators on the right drop one at a time in the order listed (leftmost first) until what remains fits.

Line 1 leftmost (only when vi keybindings are active): a reverse-video mode block — ` I ` insert (green), ` N ` normal (red), ` V ` visual (magenta), ` R ` replace (yellow). Auto-skipped under emacs keybindings; toggle with `set -g _angler_show_vi_mode 0`.

Line 2: prompt symbol (default `❯`).

## Requirements

- fish ≥ 4.0
- macOS or Linux (the BSDs probably work; Windows is untested)

## Install

angler is two halves: fish files (the prompt rendering) and a Rust daemon (`angler-daemon`, the git status engine).
Pick a path:

### fisher (recommended)

```fish
fisher install tymcauley/angler
```

This installs the fish files.
On install or `fisher update`, angler checks for `angler-daemon` on `$PATH` and prints what to do if it's missing or out of date — usually one of:

```fish
cargo install --locked --git https://github.com/tymcauley/angler --tag v0.1.0
```

or grab a prebuilt tarball from [GitHub Releases](https://github.com/tymcauley/angler/releases) and put `angler-daemon` somewhere on `$PATH`.

Then `exec fish`.

### From source (for hacking on angler)

```sh
git clone https://github.com/tymcauley/angler ~/code/angler
cd ~/code/angler
make install
```

Builds the daemon to `~/.cargo/bin/angler-daemon` and **symlinks** the fish files into `$__fish_config_dir` (typically `~/.config/fish/`).
Symlinks rather than copies, so editing the repo files and running `exec fish` picks up changes without reinstalling.

For a non-default fish config dir: `make install FISH_CONFIG_DIR=/some/path`.

### Coexisting with another prompt

If you already have Hydro / Tide / Starship / etc., they've already defined `fish_prompt`, and fish's autoloader won't overwrite it.
Remove the existing prompt first:

```sh
fisher remove jorgebucaran/hydro    # or whatever you have
```

then `exec fish`.

## Configuration

Knobs are fish variables you override.
All of them work with `set -g` in `config.fish`: symbols, colors, and toggles are read at prompt-render time, and the daemon spawns lazily on the first prompt render — after `config.fish` has run — so the daemon-tuning knobs (`_angler_dirty_deadline_ms`, `_angler_log_file`) are read in time too.

Symbols:

```fish
set -g _angler_symbol_modified  '*'   # red, unstaged tracked changes
set -g _angler_symbol_staged    '+'   # green, changes added but not committed
set -g _angler_symbol_untracked '?'   # yellow, untracked files
set -g _angler_symbol_conflict  '!'   # red bold, merge/rebase conflict
set -g _angler_symbol_unknown   '?'   # yellow, dirty deadline expired
set -g _angler_symbol_ahead     '↑'   # commits ahead of upstream
set -g _angler_symbol_behind    '↓'   # commits behind upstream
set -g _angler_symbol_gone      '↯'   # red, upstream branch is gone
set -g _angler_symbol_stash     '≡'   # blue, stash count
set -g _angler_symbol_submodule 's'   # yellow, count of submodules with changes
set -g _angler_symbol_prompt    '❯'
```

Colors are `set_color` arguments stored as fish lists, so multi-arg styles work directly.
Most defaults are bolded so the overall weight reads consistent; time and duration intentionally stay plain so they read as background metadata.

```fish
set -g _angler_color_path       cyan          # path prefix (truncated parent dirs)
set -g _angler_color_path_tail  cyan --bold   # last path component, emphasized
set -g _angler_color_branch     yellow --bold
set -g _angler_color_prompt     green --bold  # the line-2 prompt symbol
set -g _angler_color_time       brblack       # plain, intentionally
set -g _angler_color_duration   brblack       # plain, matches time
```

Drop `--bold` from any of these if you want a less-heavy look.

Toggles (1 to show, anything else to hide):

```fish
set -g _angler_show_ahead_behind 1
set -g _angler_show_stash        1
set -g _angler_show_submodule    1
set -g _angler_show_operation    1
set -g _angler_show_exit_code    1
set -g _angler_show_time         1
set -g _angler_show_cmd_duration 1
set -g _angler_show_ssh          1
set -g _angler_show_venv         1
set -g _angler_show_direnv       1
set -g _angler_show_vi_mode      1   # auto-skipped under emacs keybindings
set -g _angler_cmd_duration_threshold_ms 1000   # only show duration past this
```

Environmental-indicator colors:

```fish
set -g _angler_color_ssh    red --bold
set -g _angler_color_venv   blue
set -g _angler_color_direnv green
```

Vi-mode block (line 1 leftmost, rendered by `fish_mode_prompt` when vi keybindings are active):

```fish
set -g _angler_symbol_vi_insert  ' I '   # insert mode
set -g _angler_symbol_vi_default ' N '   # normal mode
set -g _angler_symbol_vi_visual  ' V '   # visual mode
set -g _angler_symbol_vi_replace ' R '   # replace mode

set -g _angler_color_vi_insert  green   --reverse --bold
set -g _angler_color_vi_default red     --reverse --bold
set -g _angler_color_vi_visual  magenta --reverse --bold
set -g _angler_color_vi_replace yellow  --reverse --bold
```

Drop `--reverse` from the colors for plain colored letters; set the symbols to `'[I]'`/`'[N]'`/etc. for the older bracket style.

Daemon tuning (passed as flags when the daemon spawns):

```fish
set -g _angler_dirty_deadline_ms 200   # how long to wait synchronously before
                                   # falling back to a deferred result
set -g _angler_log_file ""             # path to the daemon log file; empty
                                   # disables logging entirely (default)
```

The daemon spawns lazily on the first prompt render, so these are read after `config.fish` has run — `set -g` works.
Per-session overrides in the running shell don't take effect until the daemon is restarted (`exec fish`), since they're read once at spawn time.

## Debugging

If the prompt feels off — slow, stale, missing git info — point the daemon at a log file:

```fish
set -g _angler_log_file ~/.cache/angler.log
exec fish
```

(`exec fish` because the daemon reads `_angler_log_file` once at spawn time.)

Each line is `<rfc3339-timestamp> [<daemon-pid>] <event> key=value …`.
Events include `start`, `parent_death_armed` (which kernel mechanism — `prctl` on Linux, `kqueue` on macOS — is watching for fish exit), `request`, `watch` / `unwatch`, `watcher_fire`, `dirty_walk` (with `dur_ms` walk timing), `dirty_deferred` (deadline path), `walk_resolved` / `walk_dropped` / `walk_coalesced` / `walk_pending_kicked` (the coalescing pipeline), `status` / `status_skip` (idempotent write vs. unchanged-bytes skip), `parent_died` (fish exited; daemon shutting down), and `state_dir_cleaned` (per-PID state dir removed on the way out).

```
2026-05-04T18:26:40.659Z [4592] request pwd=/Users/tynan/code/angler
2026-05-04T18:26:40.661Z [4592] watch git_dir=/Users/tynan/code/angler/.git
2026-05-04T18:26:40.662Z [4592] dirty_walk dur_ms=2 result=*u
2026-05-04T18:26:40.663Z [4592] status branch=main dirty=*u ahead=0 behind=0 upstream= stash=0 op= dur_ms=3
```

You can use a fixed path across all shells (each daemon prefixes its lines with its PID) or per-shell logs via `set -U _angler_log_file /tmp/angler-$fish_pid.log`.

## Uninstall

```sh
make uninstall                # removes the symlinks
cargo uninstall angler   # removes the daemon binary
```

## How it works

A per-shell daemon spawned on the first prompt render reads PWD changes from a FIFO, computes git status via gix, writes a NUL-delimited status file, and sends SIGUSR1 to fish — whose `--on-signal` handler calls `commandline -f repaint`.
`fish_prompt` also pokes the daemon on every prompt render, so worktree-only changes (an editor saving a file between cds) get caught the next time anything redraws the prompt; the daemon dedupes its writes against the last-written bytes, so this per-render kick doesn't form a SIGUSR1 → repaint → request → SIGUSR1 loop.
The daemon also watches `.git/` (and `.git/refs/` recursively) via `notify-debouncer-full`, so external git operations trigger the same render path.
The dirty check is bounded by a deadline; on huge repos it returns "unknown" synchronously and the prompt updates again once the background scan finishes.
A persistent worker thread serializes all gix walks: bursts (e.g., `git checkout` rewriting many files) collapse to one walk plus at most one follow-up rather than spawning concurrent walks.
The daemon's state lives at a deterministic per-PID path (under `$XDG_RUNTIME_DIR` on Linux, `$TMPDIR` on macOS), so an `exec fish` cleanly adopts the existing daemon — same kernel task, same PID, same state — instead of orphaning it and respawning.
Daemon cleanup is automatic via kernel-level parent-death detection (`PR_SET_PDEATHSIG` on Linux, `kqueue` + `EVFILT_PROC` on macOS) — no polling, no orphans on shell exit, even when fish dies via `kill -9`.
If the daemon dies (panic, OOM, manual kill), fish respawns it before the next request rather than hanging on the FIFO write or rendering an empty git block forever.

Both sides of the wire are NUL-delimited (matching `find -print0` framing — robust against paths with embedded newlines or non-UTF-8 bytes) and prefixed with a wire-version sentinel `AN1`.
The request is `AN1\0<path>\0`; the response is `AN1\0<requested-path>\0<branch>\0<ahead>\0<behind>\0<dirty>\0<operation>\0<upstream>\0<stash>\0<submodules>\0`.
The version is checked strictly on both sides — old fish hitting a new daemon (or vice versa) degrades to "no git block" rather than silently misparsing.
`branch` is empty when the path isn't a git repo; `dirty` is `0` for clean, `?` for unknown, or some combination of `+` (staged), `*` (modified), `u` (untracked), `!` (conflict); `operation` is a label like `rebasing`/`merging` or empty; `upstream` is `gone` or empty; `stash` is the stash count; `submodules` is the count of submodules with changes (subject to the user's `diff.ignoreSubmodules`).
`fish_prompt` ignores responses whose path doesn't match the current `$PWD`, so stale fires during rapid `cd` are harmless.

## Development

```sh
make            # list targets
make check      # fmt-check + clippy + tests (full pre-push verification)
make test       # all tests (Rust integration + fish render)
```

The Rust tests spawn the daemon as a subprocess and drive it via the FIFO.
The fish tests write hand-crafted status files and assert on the rendered substring.

### Releasing

Three places in the tree carry the version literal — `Cargo.toml`, `functions/_angler_install.fish` (the `expected` value the plugin checks), and `README.md` (the `--tag vX.Y.Z` install command).
`scripts/release.sh` bumps all of them in lockstep, regenerates `Cargo.lock`, and runs the full check.
It refuses to run on a dirty tree and stops short of any irreversible step so the diff can be reviewed first.

```sh
scripts/release.sh 0.2.0    # bump literals + run make check
git diff                    # review
git commit -am "Release v0.2.0"
git tag -a v0.2.0 -m "v0.2.0"
git push && git push origin v0.2.0
```

The release workflow runs on tag push and produces tarballs at `https://github.com/tymcauley/angler/releases/tag/v0.2.0`.

The wire-version sentinel (`AN1` in `src/main.rs` and the fish files) is independent — bump it only when the protocol actually changes.
A normal release leaves it alone.
