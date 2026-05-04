use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, FileIdMap, new_debouncer};

const DEFAULT_DIRTY_DEADLINE: Duration = Duration::from_millis(200);
const DEBOUNCE: Duration = Duration::from_millis(150);

enum Event {
    Request(PathBuf),
    WatcherFired,
    /// A background dirty computation finished after the deadline expired.
    /// `generation` and `pwd` together identify which request this resolution
    /// belongs to — newer generations or PWD changes invalidate it.
    DirtyResolved {
        generation: u64,
        pwd: PathBuf,
        result: DirtyState,
    },
    Eof,
}

fn main() {
    let mut fish_pid: Option<i32> = None;
    let mut status_file: Option<PathBuf> = None;
    let mut request_fifo: Option<PathBuf> = None;
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
            "--status-file" => {
                status_file = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--request-fifo" => {
                request_fifo = args.get(i + 1).map(PathBuf::from);
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
            other => {
                eprintln!("fish-prompt-daemon: unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let fish_pid = fish_pid.expect("--fish-pid required");
    let status_file = status_file.expect("--status-file required");
    let request_fifo = request_fifo.expect("--request-fifo required");

    if let Some(path) = log_file.as_ref()
        && let Err(e) = install_logger(path)
    {
        eprintln!(
            "fish-prompt-daemon: --log-file open failed for {}: {e}",
            path.display(),
        );
    }

    log::info!(
        "start fish_pid={fish_pid} status_file={} request_fifo={} dirty_deadline_ms={}",
        status_file.display(),
        request_fifo.display(),
        dirty_deadline.as_millis(),
    );

    spawn_watchdog();

    let (tx, rx) = mpsc::channel();

    spawn_fifo_reader(request_fifo, tx.clone());

    let watch_tx = tx.clone();
    let mut debouncer: Debouncer<notify::RecommendedWatcher, FileIdMap> =
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
    // Bumped on every Request. A deferred dirty result (DirtyResolved) is
    // discarded if its generation no longer matches — the user has moved on
    // and the answer is for a stale prompt state.
    let mut current_generation: u64 = 0;

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
                let new_git_dir = repo.as_ref().map(|r| r.git_dir().to_path_buf());
                if new_git_dir != watched_git_dir {
                    if let Some(old) = watched_git_dir.take() {
                        unwatch_repo(&mut debouncer, &old);
                        log::info!("unwatch git_dir={}", old.display());
                    }
                    if let Some(ref new) = new_git_dir {
                        match watch_repo(&mut debouncer, new) {
                            Ok(()) => {
                                watched_git_dir = Some(new.clone());
                                log::info!("watch git_dir={}", new.display());
                            }
                            Err(e) => {
                                log::error!("watch_failed git_dir={} err={e}", new.display());
                            }
                        }
                    }
                }

                let status = repo.as_ref().map(|r| {
                    let dirty =
                        compute_dirty(dirty_deadline, current_generation, path.clone(), tx.clone());
                    compute_status_for_repo(r, dirty)
                });
                let _ = write_status_file(&status_file, &path, status.as_ref());
                log_status_event(status.as_ref(), start.elapsed());
                signal_fish(fish_pid);
            }
            Event::WatcherFired => {
                let Some(pwd) = current_pwd.clone() else {
                    continue;
                };
                let start = Instant::now();
                log::info!("watcher_fire pwd={}", pwd.display());
                let status = gix::discover(&pwd).ok().map(|r| {
                    let dirty =
                        compute_dirty(dirty_deadline, current_generation, pwd.clone(), tx.clone());
                    compute_status_for_repo(&r, dirty)
                });
                let _ = write_status_file(&status_file, &pwd, status.as_ref());
                log_status_event(status.as_ref(), start.elapsed());
                signal_fish(fish_pid);
            }
            Event::DirtyResolved {
                generation,
                pwd,
                result,
            } => {
                // Drop stale results: the user has moved on, or the request
                // we ran for has been superseded.
                if generation != current_generation {
                    log::info!(
                        "dirty_dropped reason=stale_generation pwd={}",
                        pwd.display(),
                    );
                    continue;
                }
                if Some(&pwd) != current_pwd.as_ref() {
                    log::info!("dirty_dropped reason=stale_pwd pwd={}", pwd.display());
                    continue;
                }
                let Some(repo) = gix::discover(&pwd).ok() else {
                    log::info!("dirty_dropped reason=repo_missing pwd={}", pwd.display());
                    continue;
                };
                let start = Instant::now();
                let result_label = result.encoded();
                let status = compute_status_for_repo(&repo, result);
                let _ = write_status_file(&status_file, &pwd, Some(&status));
                log::info!("dirty_resolved result={result_label}");
                log_status_event(Some(&status), start.elapsed());
                signal_fish(fish_pid);
            }
        }
    }
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
                eprintln!("fish-prompt-daemon: open fifo: {e}");
                tx.send(Event::Eof).ok();
                return;
            }
        };
        let reader = BufReader::new(fifo);
        for line in reader.lines() {
            let Ok(line) = line else {
                tx.send(Event::Eof).ok();
                return;
            };
            let path = PathBuf::from(line.trim());
            if !path.as_os_str().is_empty() && tx.send(Event::Request(path)).is_err() {
                return;
            }
        }
        tx.send(Event::Eof).ok();
    });
}

fn spawn_watchdog() {
    let initial_ppid = unsafe { libc::getppid() };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // When fish dies, this process gets reparented to init/launchd, so
            // getppid() returns a different value than at startup.
            let current = unsafe { libc::getppid() };
            if current != initial_ppid {
                log::info!("parent_died initial_ppid={initial_ppid} current_ppid={current}");
                std::process::exit(0);
            }
        }
    });
}

// Watch the small set of paths inside .git that meaningfully change git status:
//   - .git itself non-recursively → catches HEAD, index, packed-refs, MERGE_HEAD
//     (and avoids watching .git/objects which is large and noisy).
//   - .git/refs recursively → catches local + remote ref tip moves.
// A few inotify watches per repo regardless of repo size.
fn watch_repo(
    debouncer: &mut Debouncer<notify::RecommendedWatcher, FileIdMap>,
    git_dir: &Path,
) -> Result<(), notify::Error> {
    debouncer.watch(git_dir, RecursiveMode::NonRecursive)?;
    let refs = git_dir.join("refs");
    if refs.exists() {
        debouncer.watch(&refs, RecursiveMode::Recursive)?;
    }
    Ok(())
}

fn unwatch_repo(debouncer: &mut Debouncer<notify::RecommendedWatcher, FileIdMap>, git_dir: &Path) {
    let _ = debouncer.unwatch(git_dir);
    let _ = debouncer.unwatch(git_dir.join("refs"));
}

struct Status {
    branch: String,
    upstream: UpstreamState,
    dirty: DirtyState,
    operation: Option<&'static str>,
    stash: u32,
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
    fn is_full(self) -> bool {
        self.staged && self.modified && self.untracked && self.conflict
    }
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

fn compute_status_for_repo(repo: &gix::Repository, dirty: DirtyState) -> Status {
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
        dirty,
        operation: detect_operation(repo.git_dir()),
        stash: count_stashes(repo.git_dir()),
    }
}

// Counts entries in the stash reflog. Each `git stash push` appends one line;
// `stash pop`/`drop` rewrites the file with one fewer line. Absent file means
// zero stashes (the normal case for repos that have never been stashed).
fn count_stashes(git_dir: &Path) -> u32 {
    match std::fs::read_to_string(git_dir.join("logs/refs/stash")) {
        Ok(content) => u32::try_from(content.lines().count()).expect("stash count overflows u32"),
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
                u32::try_from(walk.filter_map(Result::ok).count())
                    .expect("ahead/behind count overflows u32")
            })
    };

    UpstreamState::Tracking {
        ahead: count_walk(head_id, upstream_id),
        behind: count_walk(upstream_id, head_id),
    }
}

// Spawns the actual dirty computation on a background thread and blocks the
// main thread for at most `deadline`.
//
// If the worker finishes in time, returns its result.
// If the deadline expires, returns Unknown and lets the worker keep running;
// when it eventually finishes, it sends an Event::DirtyResolved through
// `main_tx` so the main loop can re-emit the status. `generation` and `pwd`
// are echoed back so the receiver can drop stale answers.
//
// A short-circuit: if `deadline == ZERO` we skip the work entirely (used by
// the DirtyResolved handler when re-rendering with an already-computed dirty).
fn compute_dirty(
    deadline: Duration,
    generation: u64,
    pwd: PathBuf,
    main_tx: mpsc::Sender<Event>,
) -> DirtyState {
    let repo_path = pwd.clone();
    let (tx, rx) = mpsc::channel::<DirtyState>();

    std::thread::spawn(move || {
        let result = match gix::discover(&repo_path).ok() {
            Some(r) => compute_dirty_unbounded(&r),
            None => DirtyState::Unknown,
        };
        // Try to deliver synchronously first. If the main thread is no longer
        // waiting (deadline already passed and rx is dropped), fall back to
        // the main channel so the deferred result still lands.
        if tx.send(result.clone()).is_err() {
            main_tx
                .send(Event::DirtyResolved {
                    generation,
                    pwd,
                    result,
                })
                .ok();
        }
    });

    if let Ok(result) = rx.recv_timeout(deadline) {
        result
    } else {
        log::info!("dirty_deferred deadline_ms={}", deadline.as_millis());
        DirtyState::Unknown
    }
}

// Iterates gix's parallel status engine without a deadline-driven interrupt.
// Drains the iterator (or short-circuits when all four flags are observed)
// and returns Clean / Dirty(flags) / Unknown (only on gix errors).
fn compute_dirty_unbounded(repo: &gix::Repository) -> DirtyState {
    let Ok(platform) = repo.status(gix::progress::Discard) else {
        return DirtyState::Unknown;
    };

    let Ok(iter) = platform.into_iter(None) else {
        return DirtyState::Unknown;
    };

    let mut flags = DirtyFlags::default();
    for item in iter.flatten() {
        classify_item(item, &mut flags);
        if flags.is_full() {
            break;
        }
    }

    if flags.any() {
        DirtyState::Dirty(flags)
    } else {
        DirtyState::Clean
    }
}

fn classify_item(item: gix::status::Item, flags: &mut DirtyFlags) {
    use gix::status::Item;
    use gix::status::index_worktree::Item as IWItem;
    use gix::status::plumbing::index_as_worktree::EntryStatus;

    match item {
        Item::TreeIndex(_) => flags.staged = true,
        Item::IndexWorktree(IWItem::Modification { status, .. }) => match status {
            EntryStatus::Conflict { .. } => flags.conflict = true,
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

fn write_status_file(
    path: &Path,
    request_path: &Path,
    status: Option<&Status>,
) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        // Each field NUL-terminated. 8 fields:
        //   request_path, branch, ahead, behind, dirty, operation, upstream, stash
        // For non-repos, the last 7 fields are empty. ahead/behind are "0"
        // when no upstream or upstream is gone; the upstream field carries
        // the qualitative signal.
        f.write_all(request_path.as_os_str().as_bytes())?;
        f.write_all(b"\0")?;
        if let Some(s) = status {
            let (ahead, behind, upstream_label) = match s.upstream {
                UpstreamState::Tracking { ahead, behind } => (ahead, behind, ""),
                UpstreamState::Gone => (0, 0, "gone"),
                UpstreamState::None => (0, 0, ""),
            };
            write!(
                f,
                "{}\0{}\0{}\0{}\0{}\0{}\0{}\0",
                s.branch,
                ahead,
                behind,
                s.dirty.encoded(),
                s.operation.unwrap_or(""),
                upstream_label,
                s.stash,
            )?;
        } else {
            f.write_all(b"\0\0\0\0\0\0\0")?;
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
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
                "status branch={} dirty={} ahead={ahead} behind={behind} upstream={upstream} stash={} op={} dur_ms={dur_ms}",
                s.branch,
                s.dirty.encoded(),
                s.stash,
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
