use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

const DEFAULT_DIRTY_DEADLINE: Duration = Duration::from_millis(200);
const DEBOUNCE: Duration = Duration::from_millis(150);

enum Event {
    Request(PathBuf),
    WatcherFired,
    /// A walk finished. `generation` and `pwd` together identify which
    /// request kicked the walk — newer generations or PWD changes mean the
    /// answer is for a stale state and gets dropped.
    WalkComplete {
        generation: u64,
        pwd: PathBuf,
        result: WalkResult,
    },
    Eof,
}

/// Sent from the main thread to the persistent worker thread. The worker
/// processes one of these at a time (serializing all walks). When it
/// finishes it always emits `Event::WalkComplete` to the main channel; if
/// `deadline_tx` is present it ALSO best-effort-delivers the result there
/// for an opportunistic synchronous wait on the request entry path.
struct WalkRequest {
    generation: u64,
    pwd: PathBuf,
    deadline_tx: Option<mpsc::Sender<WalkResult>>,
}

#[derive(Clone)]
struct WalkResult {
    dirty: DirtyState,
    /// Count of submodules gix reports any change for, after honoring the
    /// user's `diff.ignoreSubmodules` config. Independent of `dirty` so a
    /// repo with clean files but a HEAD-moved submodule reports `0` for
    /// dirty and `1` here.
    submodules: u32,
}

fn main() {
    let mut fish_pid: Option<i32> = None;
    let mut state_dir: Option<PathBuf> = None;
    let mut log_file: Option<PathBuf> = None;
    let mut dirty_deadline = DEFAULT_DIRTY_DEADLINE;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--fish-pid" => {
                fish_pid = args.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "--state-dir" => {
                state_dir = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--log-file" => {
                log_file = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--dirty-deadline-ms" => {
                if let Some(ms) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                    dirty_deadline = Duration::from_millis(ms);
                }
                i += 2;
            }
            "--version" | "-V" => {
                println!("angler-daemon {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => {
                eprintln!("angler-daemon: unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let fish_pid = fish_pid.expect("--fish-pid required");
    let state_dir = state_dir.expect("--state-dir required");
    let request_fifo = state_dir.join("req");
    let status_file = state_dir.join("status");

    if let Some(path) = log_file.as_ref()
        && let Err(e) = install_logger(path)
    {
        eprintln!(
            "angler-daemon: --log-file open failed for {}: {e}",
            path.display(),
        );
    }

    log::info!(
        "start fish_pid={fish_pid} state_dir={} dirty_deadline_ms={}",
        state_dir.display(),
        dirty_deadline.as_millis(),
    );

    arm_parent_death(state_dir.clone());

    let (tx, rx) = mpsc::channel();

    spawn_fifo_reader(request_fifo, tx.clone());

    // Persistent worker for gix walks. One worker = at most one walk in
    // flight, ever. Coalescing falls out of the `walk_inflight` state we
    // track in the main loop below.
    let (work_tx, work_rx) = mpsc::channel::<WalkRequest>();
    spawn_walk_worker(work_rx, tx.clone());

    let watch_tx = tx.clone();
    let mut debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache> =
        new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            // We don't care which path fired — main thread re-computes for the
            // current PWD. (See the path-match guard in fish_prompt for why
            // this is safe even if a non-current repo's events trigger us.)
            if let Ok(events) = result
                && !events.is_empty()
            {
                watch_tx.send(Event::WatcherFired).ok();
            }
        })
        .expect("create debouncer");

    // The current PWD fish has told us about, and the git_dir of its repo (if
    // any). We render against current_pwd and only watch one repo at a time —
    // we never display anything for a previously-visited repo, so keeping its
    // watches alive would just produce wasted re-renders that fish_prompt's
    // path-match guard discards.
    let mut current_pwd: Option<PathBuf> = None;
    let mut watched_git_dir: Option<PathBuf> = None;
    // Bumped on every Request. A WalkComplete whose generation no longer
    // matches is dropped — the user has moved on and the answer is for a
    // stale prompt state.
    let mut current_generation: u64 = 0;
    // Bytes of the most recent status file write. Used to skip redundant
    // writes + SIGUSR1s when a fresh recompute lands on the same state.
    // Without this, fish_prompt firing a request on every render would
    // form a SIGUSR1 → repaint → request → SIGUSR1 loop.
    let mut last_status_bytes: Option<Vec<u8>> = None;
    // Generation of the walk currently being processed by the worker, if
    // any. While Some, new Request/WatcherFired events do not kick a walk;
    // they just set walk_pending. When the walk completes, if pending was
    // set, we kick exactly one more walk for the current state.
    let mut walk_inflight: Option<u64> = None;
    let mut walk_pending = false;

    while let Ok(event) = rx.recv() {
        match event {
            Event::Eof => break,
            Event::Request(path) => {
                let start = Instant::now();
                log::info!("request pwd={}", path.display());
                current_pwd = Some(path.clone());
                current_generation += 1;
                let repo = gix::discover(&path).ok();

                // Install the watch BEFORE rendering. Otherwise a fish caller
                // can observe the post-request status file, immediately mutate
                // the repo (e.g., `git commit`), and have that change land in
                // the gap between rendering and watch_repo — so the daemon
                // never sees the event and the prompt goes stale.
                swap_repo_watch(&mut debouncer, repo.as_ref(), &mut watched_git_dir);

                let Some(dirty) = maybe_kick_walk(
                    &work_tx,
                    &mut walk_inflight,
                    &mut walk_pending,
                    current_generation,
                    path.clone(),
                    dirty_deadline,
                ) else {
                    // Coalesced: a walk is already in flight. Don't write
                    // status; the WalkComplete handler will update once the
                    // walk lands. Writing dirty=? here would briefly show
                    // ? in the prompt and create a real→?→real flicker.
                    continue;
                };
                let status = repo.as_ref().map(|r| compute_status_for_repo(r, dirty));
                write_and_signal(
                    &status_file,
                    &path,
                    status.as_ref(),
                    start.elapsed(),
                    fish_pid,
                    &mut last_status_bytes,
                );
            }
            Event::WatcherFired => {
                let Some(pwd) = current_pwd.clone() else {
                    continue;
                };
                let start = Instant::now();
                log::info!("watcher_fire pwd={}", pwd.display());

                let Some(dirty) = maybe_kick_walk(
                    &work_tx,
                    &mut walk_inflight,
                    &mut walk_pending,
                    current_generation,
                    pwd.clone(),
                    dirty_deadline,
                ) else {
                    continue;
                };
                let status = gix::discover(&pwd)
                    .ok()
                    .map(|r| compute_status_for_repo(&r, dirty));
                write_and_signal(
                    &status_file,
                    &pwd,
                    status.as_ref(),
                    start.elapsed(),
                    fish_pid,
                    &mut last_status_bytes,
                );
            }
            Event::WalkComplete {
                generation,
                pwd,
                result,
            } => {
                walk_inflight = None;

                let still_current =
                    generation == current_generation && Some(&pwd) == current_pwd.as_ref();
                if still_current {
                    log::info!(
                        "walk_resolved result={} submodules={}",
                        result.dirty.encoded(),
                        result.submodules,
                    );
                    if let Ok(repo) = gix::discover(&pwd) {
                        let start = Instant::now();
                        let status = compute_status_for_repo(&repo, result);
                        write_and_signal(
                            &status_file,
                            &pwd,
                            Some(&status),
                            start.elapsed(),
                            fish_pid,
                            &mut last_status_bytes,
                        );
                    } else {
                        log::info!("walk_dropped reason=repo_missing pwd={}", pwd.display());
                    }
                } else {
                    log::info!(
                        "walk_dropped reason=stale gen={generation} current_gen={current_generation}",
                    );
                }

                // Coalesced requests piled up while we were busy. Kick one
                // more walk for whatever the current state is now. Async
                // (no deadline_tx); the result lands via WalkComplete.
                if walk_pending {
                    walk_pending = false;
                    if let Some(pending_pwd) = current_pwd.clone() {
                        log::info!("walk_pending_kicked gen={current_generation}");
                        walk_inflight = Some(current_generation);
                        let _ = work_tx.send(WalkRequest {
                            generation: current_generation,
                            pwd: pending_pwd,
                            deadline_tx: None,
                        });
                    }
                }
            }
        }
    }
}

// Decide what to do for a "we want fresh dirty info" trigger:
//   - If the worker is idle: kick a walk and synchronously wait up to
//     `deadline` for the result. Return Some(dirty); caller writes status.
//   - If the worker is busy: record that another walk is wanted (the
//     in-flight walk's completion handler will kick one more walk) and
//     return None; caller skips the status write so we don't flicker the
//     prompt to "?" while there's already a recent real result on file.
fn maybe_kick_walk(
    work_tx: &mpsc::Sender<WalkRequest>,
    walk_inflight: &mut Option<u64>,
    walk_pending: &mut bool,
    generation: u64,
    pwd: PathBuf,
    deadline: Duration,
) -> Option<WalkResult> {
    if walk_inflight.is_some() {
        log::info!("walk_coalesced gen={generation}");
        *walk_pending = true;
        return None;
    }
    let (sync_tx, sync_rx) = mpsc::channel();
    *walk_inflight = Some(generation);
    let _ = work_tx.send(WalkRequest {
        generation,
        pwd,
        deadline_tx: Some(sync_tx),
    });
    Some(sync_rx.recv_timeout(deadline).unwrap_or_else(|_| {
        log::info!("dirty_deferred deadline_ms={}", deadline.as_millis());
        WalkResult {
            dirty: DirtyState::Unknown,
            submodules: 0,
        }
    }))
}

// Swap which repo's `.git/` we watch. Idempotent if `git_dir` hasn't
// changed since last call.
fn swap_repo_watch(
    debouncer: &mut Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    repo: Option<&gix::Repository>,
    watched_git_dir: &mut Option<PathBuf>,
) {
    let new_git_dir = repo.map(|r| r.git_dir().to_path_buf());
    if new_git_dir == *watched_git_dir {
        return;
    }
    if let Some(old) = watched_git_dir.take() {
        unwatch_repo(debouncer, &old);
        log::info!("unwatch git_dir={}", old.display());
    }
    if let Some(ref new) = new_git_dir {
        match watch_repo(debouncer, new) {
            Ok(()) => {
                *watched_git_dir = Some(new.clone());
                log::info!("watch git_dir={}", new.display());
            }
            Err(e) => {
                log::error!("watch_failed git_dir={} err={e}", new.display());
            }
        }
    }
}

// Persistent worker thread. Pulls one WalkRequest at a time and runs the
// gix walk. Always reports completion to the main loop (even when
// deadline_tx received the result first), so the main loop can update its
// walk_inflight state and kick a pending walk if there is one.
fn spawn_walk_worker(work_rx: mpsc::Receiver<WalkRequest>, main_tx: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        while let Ok(req) = work_rx.recv() {
            let WalkRequest {
                generation,
                pwd,
                deadline_tx,
            } = req;
            let walk_start = Instant::now();
            let result = match gix::discover(&pwd).ok() {
                Some(r) => compute_dirty_unbounded(&r),
                None => WalkResult {
                    dirty: DirtyState::Unknown,
                    submodules: 0,
                },
            };
            log::info!(
                "dirty_walk dur_ms={} result={} submodules={}",
                walk_start.elapsed().as_millis(),
                result.dirty.encoded(),
                result.submodules,
            );
            // Best-effort sync delivery. May fail if the main loop already
            // timed out and dropped its rx; that's fine — the main_tx send
            // below covers the recovery path.
            if let Some(tx) = deadline_tx {
                let _ = tx.send(result.clone());
            }
            main_tx
                .send(Event::WalkComplete {
                    generation,
                    pwd,
                    result,
                })
                .ok();
        }
    });
}

fn signal_fish(pid: i32) {
    // SAFETY: kill(2) with a valid signal number is safe; pid is i32.
    unsafe { libc::kill(pid, libc::SIGUSR1) };
}

fn spawn_fifo_reader(fifo_path: PathBuf, tx: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        // Open RDWR so reads don't EOF every time fish closes its write end.
        let fifo = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo_path)
        {
            Ok(f) => f,
            Err(e) => {
                log::error!("fifo_open_failed err={e}");
                eprintln!("angler-daemon: open fifo: {e}");
                tx.send(Event::Eof).ok();
                return;
            }
        };
        // NUL-delimited framing (matches `find -print0` / `xargs -0` style).
        // Reading raw bytes keeps non-UTF-8 path bytes intact and tolerates
        // embedded newlines; both are legal in Unix paths and would corrupt
        // newline-delimited framing.
        //
        // Each request is two tokens: the wire-version sentinel followed by
        // the path. A first token that doesn't match WIRE_VERSION is dropped
        // — old fish hitting a new daemon will fall here, and degraded
        // "no git block" is preferable to interpreting the path as a version.
        let mut reader = BufReader::new(fifo);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(0, &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if buf.last() == Some(&0) {
                        buf.pop();
                    }
                    if buf.is_empty() {
                        continue;
                    }
                    if buf != WIRE_VERSION {
                        log::warn!(
                            "fifo_request_unknown_version got={}",
                            String::from_utf8_lossy(&buf)
                        );
                        continue;
                    }
                    buf.clear();
                    match reader.read_until(0, &mut buf) {
                        Ok(0) => break,
                        Ok(_) => {
                            if buf.last() == Some(&0) {
                                buf.pop();
                            }
                            if buf.is_empty() {
                                continue;
                            }
                            let path = PathBuf::from(std::ffi::OsStr::from_bytes(&buf));
                            if tx.send(Event::Request(path)).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            log::error!("fifo_read_failed err={e}");
                            break;
                        }
                    }
                }
                Err(e) => {
                    log::error!("fifo_read_failed err={e}");
                    break;
                }
            }
        }
        tx.send(Event::Eof).ok();
    });
}

// Arm the kernel to notify us when our parent (fish) dies, then sleep
// efficiently until that happens. Linux uses PR_SET_PDEATHSIG: the kernel
// sends a signal when the parent thread group exits. macOS uses kqueue
// EVFILT_PROC | NOTE_EXIT: a kqueue fd becomes readable on parent exit.
// Either way, no polling — the daemon is genuinely idle between events,
// not waking every 500ms.
//
// Both paths handle the registration race (parent already dead when we
// arm) by re-checking after registration. Both clean up state_dir before
// exit so a kill -9 of fish doesn't leak files.
//
// Fail loud on registration failure: a daemon we can't shut down on
// parent death is worse than no daemon at all.
fn arm_parent_death(state_dir: PathBuf) {
    let initial_ppid = unsafe { libc::getppid() };

    #[cfg(target_os = "linux")]
    arm_parent_death_linux(initial_ppid, state_dir);
    #[cfg(target_os = "macos")]
    arm_parent_death_macos(initial_ppid, state_dir);
}

#[cfg(target_os = "linux")]
fn arm_parent_death_linux(initial_ppid: i32, state_dir: PathBuf) {
    // Block SIGTERM in this (main) thread before spawning anything else;
    // future threads inherit the blocked mask, so the dedicated waiter is
    // the only consumer. sigwait requires the signal to be blocked.
    let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::sigemptyset(&mut mask);
        libc::sigaddset(&mut mask, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut())
    };
    if rc != 0 {
        log::error!(
            "parent_death_arm_failed mechanism=prctl step=pthread_sigmask err={}",
            std::io::Error::last_os_error(),
        );
        std::process::exit(1);
    }

    let waiter_dir = state_dir.clone();
    std::thread::spawn(move || {
        let mut wait_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut wait_mask);
            libc::sigaddset(&mut wait_mask, libc::SIGTERM);
        }
        let mut sig: libc::c_int = 0;
        unsafe { libc::sigwait(&wait_mask, &mut sig) };
        log::info!("parent_died");
        if std::fs::remove_dir_all(&waiter_dir).is_ok() {
            log::info!("state_dir_cleaned dir={}", waiter_dir.display());
        }
        std::process::exit(0);
    });

    // Arm. Once set, the kernel sends SIGTERM to this process when the
    // parent thread group exits.
    let rc = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) };
    if rc != 0 {
        log::error!(
            "parent_death_arm_failed mechanism=prctl step=PR_SET_PDEATHSIG err={}",
            std::io::Error::last_os_error(),
        );
        std::process::exit(1);
    }

    // Race: parent may have died before we armed. PR_SET_PDEATHSIG only
    // fires on subsequent death, so check getppid() — if it has flipped,
    // we missed the window.
    let current = unsafe { libc::getppid() };
    if current != initial_ppid {
        log::info!("parent_died_before_arm initial_ppid={initial_ppid} current_ppid={current}");
        if std::fs::remove_dir_all(&state_dir).is_ok() {
            log::info!("state_dir_cleaned dir={}", state_dir.display());
        }
        std::process::exit(0);
    }

    log::info!("parent_death_armed mechanism=prctl ppid={initial_ppid}");
}

#[cfg(target_os = "macos")]
fn arm_parent_death_macos(initial_ppid: i32, state_dir: PathBuf) {
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        log::error!(
            "parent_death_arm_failed mechanism=kqueue step=kqueue err={}",
            std::io::Error::last_os_error(),
        );
        std::process::exit(1);
    }

    // Register: notify on parent process exit, fire once.
    let change = libc::kevent {
        ident: initial_ppid as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let n = unsafe { libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    if n < 0 {
        // Most likely ESRCH: parent already gone. Treat as parent-died.
        log::info!(
            "parent_died_before_arm err={}",
            std::io::Error::last_os_error(),
        );
        if std::fs::remove_dir_all(&state_dir).is_ok() {
            log::info!("state_dir_cleaned dir={}", state_dir.display());
        }
        std::process::exit(0);
    }

    // Race: parent may have died between getppid() above and kevent
    // registration. kill(0) returns ESRCH if so.
    if unsafe { libc::kill(initial_ppid, 0) } != 0 {
        log::info!(
            "parent_died_before_arm err={}",
            std::io::Error::last_os_error(),
        );
        if std::fs::remove_dir_all(&state_dir).is_ok() {
            log::info!("state_dir_cleaned dir={}", state_dir.display());
        }
        std::process::exit(0);
    }

    log::info!("parent_death_armed mechanism=kqueue ppid={initial_ppid}");

    std::thread::spawn(move || {
        let mut out: libc::kevent = unsafe { std::mem::zeroed() };
        let n = unsafe { libc::kevent(kq, std::ptr::null(), 0, &mut out, 1, std::ptr::null()) };
        if n < 0 {
            log::error!("kevent_wait_failed err={}", std::io::Error::last_os_error(),);
            // Fall through to exit anyway — staying alive after losing
            // our death-detection channel is worse than exiting.
        }
        log::info!("parent_died");
        if std::fs::remove_dir_all(&state_dir).is_ok() {
            log::info!("state_dir_cleaned dir={}", state_dir.display());
        }
        std::process::exit(0);
    });
}

// Watch the small set of paths inside .git that meaningfully change git status:
//   - .git itself non-recursively → catches HEAD, index, packed-refs, MERGE_HEAD
//     (and avoids watching .git/objects which is large and noisy).
//   - .git/refs recursively → catches local + remote ref tip moves.
// A few inotify watches per repo regardless of repo size.
fn watch_repo(
    debouncer: &mut Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    git_dir: &Path,
) -> Result<(), notify::Error> {
    debouncer.watch(git_dir, RecursiveMode::NonRecursive)?;
    let refs = git_dir.join("refs");
    if refs.exists() {
        debouncer.watch(&refs, RecursiveMode::Recursive)?;
    }
    Ok(())
}

fn unwatch_repo(
    debouncer: &mut Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    git_dir: &Path,
) {
    let _ = debouncer.unwatch(git_dir);
    let _ = debouncer.unwatch(git_dir.join("refs"));
}

struct Status {
    branch: String,
    upstream: UpstreamState,
    dirty: DirtyState,
    operation: Option<&'static str>,
    stash: u32,
    submodules: u32,
}

enum UpstreamState {
    /// No upstream is configured for this branch (or HEAD is detached).
    None,
    /// Upstream is configured and reachable; we have ahead/behind counts.
    Tracking { ahead: u32, behind: u32 },
    /// Upstream is configured but the tracking ref isn't there — typically
    /// because the remote branch was deleted (e.g. squash-merge cleanup).
    Gone,
}

#[derive(Clone)]
enum DirtyState {
    Clean,
    Dirty(DirtyFlags),
    Unknown,
}

#[derive(Default, Copy, Clone)]
struct DirtyFlags {
    staged: bool,
    modified: bool,
    untracked: bool,
    conflict: bool,
}

impl DirtyFlags {
    fn any(self) -> bool {
        self.staged || self.modified || self.untracked || self.conflict
    }
}

impl DirtyState {
    // Wire encoding for the `dirty` field:
    //   ""  -> caller writes empty (only when not a repo)
    //   "0" -> clean
    //   "?" -> unknown (deadline expired before any item was seen)
    //   otherwise any combination of '+', '*', 'u', '!' for the four flags.
    fn encoded(&self) -> String {
        match self {
            DirtyState::Clean => "0".into(),
            DirtyState::Unknown => "?".into(),
            DirtyState::Dirty(f) => {
                let mut s = String::new();
                if f.staged {
                    s.push('+');
                }
                if f.modified {
                    s.push('*');
                }
                if f.untracked {
                    s.push('u');
                }
                if f.conflict {
                    s.push('!');
                }
                s
            }
        }
    }
}

fn compute_status_for_repo(repo: &gix::Repository, walk: WalkResult) -> Status {
    let head_name = repo.head_name().ok().flatten();
    let branch = match &head_name {
        Some(name) => name.shorten().to_string(),
        None => match repo.head_id() {
            Ok(id) => id.to_hex_with_len(7).to_string(),
            Err(_) => "(detached)".to_string(),
        },
    };

    let upstream = head_name
        .as_ref()
        .map_or(UpstreamState::None, |n| compute_upstream(repo, n.as_ref()));

    Status {
        branch,
        upstream,
        dirty: walk.dirty,
        operation: detect_operation(repo.git_dir()),
        stash: count_stashes(repo.git_dir()),
        submodules: walk.submodules,
    }
}

// Counts entries in the stash reflog. Each `git stash push` appends one line;
// `stash pop`/`drop` rewrites the file with one fewer line. Absent file means
// zero stashes (the normal case for repos that have never been stashed).
fn count_stashes(git_dir: &Path) -> u32 {
    match std::fs::read_to_string(git_dir.join("logs/refs/stash")) {
        Ok(content) => u32::try_from(content.lines().count()).unwrap_or(u32::MAX),
        Err(_) => 0,
    }
}

// Returns a short label for any in-progress git operation. Order: rebase wins
// over merge because during a rebase conflict-resolution stop, both
// rebase-merge/ and MERGE_HEAD can exist, and the user thinks of themselves
// as rebasing.
fn detect_operation(git_dir: &Path) -> Option<&'static str> {
    let exists = |sub: &str| git_dir.join(sub).exists();
    if exists("rebase-merge") || exists("rebase-apply") {
        Some("rebasing")
    } else if exists("CHERRY_PICK_HEAD") {
        Some("cherry-picking")
    } else if exists("REVERT_HEAD") {
        Some("reverting")
    } else if exists("MERGE_HEAD") {
        Some("merging")
    } else if exists("BISECT_LOG") {
        Some("bisecting")
    } else {
        None
    }
}

// Resolves the upstream state for a branch:
//   - None when no tracking config is set (or detached HEAD).
//   - Gone when tracking is configured but the tracking ref doesn't exist —
//     i.e., remote branch was deleted, typically via squash-merge cleanup.
//   - Tracking with ahead/behind counts otherwise (symmetric difference walk).
fn compute_upstream(repo: &gix::Repository, head_name: &gix::refs::FullNameRef) -> UpstreamState {
    let Some(Ok(tracking)) =
        repo.branch_remote_tracking_ref_name(head_name, gix::remote::Direction::Fetch)
    else {
        return UpstreamState::None;
    };

    let upstream_id = match repo
        .find_reference(tracking.as_ref())
        .ok()
        .and_then(|mut r| r.peel_to_id().ok())
    {
        Some(id) => id.detach(),
        None => return UpstreamState::Gone,
    };

    let head_id = match repo.head_id() {
        Ok(id) => id.detach(),
        Err(_) => return UpstreamState::None,
    };

    let count_walk = |from: gix::ObjectId, hide: gix::ObjectId| -> u32 {
        repo.rev_walk([from])
            .with_hidden([hide])
            .all()
            .ok()
            .map_or(0, |walk| {
                u32::try_from(walk.filter_map(Result::ok).count()).unwrap_or(u32::MAX)
            })
    };

    UpstreamState::Tracking {
        ahead: count_walk(head_id, upstream_id),
        behind: count_walk(upstream_id, head_id),
    }
}

// Iterates gix's parallel status engine without a deadline-driven interrupt.
// Submodule status is honored at the level the user's `diff.ignoreSubmodules`
// config admits; submodule changes don't set the dirty flags but increment
// `submodules` so they can render with their own indicator.
fn compute_dirty_unbounded(repo: &gix::Repository) -> WalkResult {
    let unknown = || WalkResult {
        dirty: DirtyState::Unknown,
        submodules: 0,
    };
    let Ok(platform) = repo.status(gix::progress::Discard) else {
        return unknown();
    };
    let Ok(iter) = platform.into_iter(None) else {
        return unknown();
    };

    let mut flags = DirtyFlags::default();
    let mut submodules: u32 = 0;
    for item in iter.flatten() {
        classify_item(item, &mut flags, &mut submodules);
    }

    let dirty = if flags.any() {
        DirtyState::Dirty(flags)
    } else {
        DirtyState::Clean
    };
    WalkResult { dirty, submodules }
}

fn classify_item(item: gix::status::Item, flags: &mut DirtyFlags, submodules: &mut u32) {
    use gix::status::Item;
    use gix::status::index_worktree::Item as IWItem;
    use gix::status::plumbing::index_as_worktree::Change;
    use gix::status::plumbing::index_as_worktree::EntryStatus;

    match item {
        Item::TreeIndex(_) => flags.staged = true,
        Item::IndexWorktree(IWItem::Modification { status, .. }) => match status {
            EntryStatus::Conflict { .. } => flags.conflict = true,
            EntryStatus::Change(Change::SubmoduleModification(_)) => {
                *submodules = submodules.saturating_add(1);
            }
            EntryStatus::Change(_) => flags.modified = true,
            EntryStatus::NeedsUpdate(_) | EntryStatus::IntentToAdd => {}
        },
        Item::IndexWorktree(IWItem::DirectoryContents { entry, .. }) => {
            if matches!(entry.status, gix::dir::entry::Status::Untracked) {
                flags.untracked = true;
            }
        }
        Item::IndexWorktree(IWItem::Rewrite { .. }) => {
            // Renames present as a deletion + addition pair gix collapses into one.
            flags.modified = true;
        }
    }
}

// Wire format: a `FP<N>\0` version sentinel followed by N NUL-terminated
// payload fields. v1 carries 9 payload fields:
//   request_path, branch, ahead, behind, dirty, operation, upstream, stash,
//   submodules
// For non-repos, the last 8 payload fields are empty. ahead/behind are "0"
// when no upstream or upstream is gone; the upstream field carries the
// qualitative signal. The version is parsed strictly: anything other than
// `AN1` is rejected. Old fish reading new daemon (or vice versa) sees a
// version mismatch and degrades cleanly to "no git block" rather than
// silently mixing schemes.
const WIRE_VERSION: &[u8] = b"AN1";

fn build_status_bytes(request_path: &Path, status: Option<&Status>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(WIRE_VERSION);
    buf.push(0);
    buf.extend_from_slice(request_path.as_os_str().as_bytes());
    buf.push(0);
    if let Some(s) = status {
        let (ahead, behind, upstream_label) = match s.upstream {
            UpstreamState::Tracking { ahead, behind } => (ahead, behind, ""),
            UpstreamState::Gone => (0, 0, "gone"),
            UpstreamState::None => (0, 0, ""),
        };
        write!(
            buf,
            "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0",
            s.branch,
            ahead,
            behind,
            s.dirty.encoded(),
            s.operation.unwrap_or(""),
            upstream_label,
            s.stash,
            s.submodules,
        )
        .expect("writing to a Vec<u8> never fails");
    } else {
        buf.extend_from_slice(b"\0\0\0\0\0\0\0\0");
    }
    buf
}

fn write_status_file_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// Writes a fresh status file and signals fish — but skips both when the new
// bytes match the previously-written bytes. The skip is what breaks the
// fish_prompt → request → SIGUSR1 → commandline-repaint → fish_prompt loop:
// once state stabilizes, the second walk produces identical bytes and we
// don't kick fish to repaint again.
fn write_and_signal(
    path: &Path,
    request_path: &Path,
    status: Option<&Status>,
    elapsed: Duration,
    fish_pid: i32,
    last_bytes: &mut Option<Vec<u8>>,
) {
    let new_bytes = build_status_bytes(request_path, status);
    if last_bytes.as_deref() == Some(new_bytes.as_slice()) {
        log::info!(
            "status_skip reason=unchanged dur_ms={}",
            elapsed.as_millis()
        );
        return;
    }
    if let Err(e) = write_status_file_atomic(path, &new_bytes) {
        log::error!("status_write_failed err={e}");
        return;
    }
    log_status_event(status, elapsed);
    signal_fish(fish_pid);
    *last_bytes = Some(new_bytes);
}

// One-line summary of the most recent status write. Always called right
// after write_status_file so dur_ms reflects the wall clock from event
// arrival to status-file rename.
fn log_status_event(status: Option<&Status>, elapsed: Duration) {
    let dur_ms = elapsed.as_millis();
    match status {
        Some(s) => {
            let (ahead, behind, upstream) = match s.upstream {
                UpstreamState::Tracking { ahead, behind } => (ahead, behind, ""),
                UpstreamState::Gone => (0, 0, "gone"),
                UpstreamState::None => (0, 0, ""),
            };
            log::info!(
                "status branch={} dirty={} ahead={ahead} behind={behind} upstream={upstream} stash={} submodules={} op={} dur_ms={dur_ms}",
                s.branch,
                s.dirty.encoded(),
                s.stash,
                s.submodules,
                s.operation.unwrap_or(""),
            );
        }
        None => log::info!("status no_repo dur_ms={dur_ms}"),
    }
}

// Custom log::Log backend so we can write to a daemon-private file with our
// own format, rather than pulling in a backend crate that imposes its own.
// Format: "<rfc3339> [<pid>] <event-from-args>". Append-mode + a Mutex around
// the file makes concurrent calls safe; with O_APPEND the kernel guarantees
// a single write_all is atomic up to PIPE_BUF, but locking also prevents
// future multi-write events from interleaving.
struct FileLogger {
    file: Mutex<File>,
    pid: u32,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = humantime::format_rfc3339_millis(SystemTime::now());
        let line = format!("{ts} [{}] {}\n", self.pid, record.args());
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

fn install_logger(path: &Path) -> std::io::Result<()> {
    let file = File::options().create(true).append(true).open(path)?;
    log::set_boxed_logger(Box::new(FileLogger {
        file: Mutex::new(file),
        pid: std::process::id(),
    }))
    .expect("logger should only be set once per process");
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}
