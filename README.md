# fish-prompt

A fast, asynchronous git prompt for fish 4.
A per-shell Rust daemon computes status with [gix](https://github.com/GitoxideLabs/gitoxide), watches `.git/` via [`notify`](https://github.com/notify-rs/notify), and pokes fish with SIGUSR1 when there's something new to render.
Branch info appears the moment you hit Enter; dirty / ahead / behind fill in shortly after; external git operations (in-editor commits, GUI clients, scripts in another window) update the prompt without you typing anything.

## Output

```
~/code/myproject main↑2 *
❯
```

- **cyan** abbreviated working directory (and `[N]` in red after it if the last command exited non-zero)
- **yellow** branch name (or 7-char SHA if detached)
- `↑N` commits ahead of upstream, `↓N` behind
- **red** `*` if the working tree is dirty
- **yellow** `?` if the dirty check couldn't finish within the deadline (default 200ms — you'll see it on huge repos with a cold disk cache)

## Requirements

- fish ≥ 4.0
- Rust toolchain for building
- macOS or Linux (the BSDs probably work; Windows is untested)

## Install

```sh
git clone <repo-url> ~/code/fish-prompt
cd ~/code/fish-prompt
make install
```

This builds and installs the daemon binary to `~/.cargo/bin/fish-prompt-daemon` and symlinks the fish files into `$XDG_CONFIG_HOME/fish/` (defaulting to `~/.config/fish/`).
Symlinks rather than copies, so editing the repo files and running `exec fish` picks up changes without reinstalling.

For a non-default fish config dir: `make install FISH_CONFIG_DIR=/some/path`.

### Coexisting with another prompt

If you already have Hydro / Tide / Starship / etc., they've already defined `fish_prompt`, and fish's autoloader won't overwrite it.
Remove the existing prompt first:

```sh
fisher remove jorgebucaran/hydro    # or whatever you have
```

then `exec fish`.

## Uninstall

```sh
make uninstall                # removes the symlinks
cargo uninstall fish-prompt   # removes the daemon binary
```

## Installing via fisher

The repo layout is fisher-compatible by accident — fisher will pick up the `conf.d/` and `functions/` files.
It just doesn't know about Rust binaries, so you still need cargo for the daemon:

```sh
fisher install /path/to/fish-prompt
cargo install --path /path/to/fish-prompt
```

The Makefile install is the same thing in one command, plus dev-friendly symlinks instead of fisher's copies.

## How it works

A per-shell daemon spawned at fish init reads PWD changes from a FIFO (one line per `cd`), computes branch / ahead-behind / dirty via gix, writes a NUL-delimited status file, and sends SIGUSR1 to fish — whose `--on-signal` handler calls `commandline -f repaint`.
The daemon also watches `.git/` (and `.git/refs/` recursively) via `notify-debouncer-full`, so external git operations trigger the same render path.
The dirty check is bounded by a deadline; on huge repos it returns "unknown" rather than blocking.
Daemon cleanup is automatic via a `getppid()` watchdog — no orphans on shell exit.

The wire protocol is five NUL-terminated fields: `<requested-path>\0<branch>\0<ahead>\0<behind>\0<dirty>\0`.
`fish_prompt` ignores responses whose path doesn't match the current `$PWD`, so stale fires during rapid `cd` are harmless.

## Development

```sh
make            # list targets
make check      # fmt-check + clippy + tests (full pre-push verification)
make test       # all tests (Rust integration + fish render)
```

The Rust tests spawn the daemon as a subprocess and drive it via the FIFO.
The fish tests write hand-crafted status files and assert on the rendered substring.
