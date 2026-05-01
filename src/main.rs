use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};

const DEFAULT_DIRTY_DEADLINE: Duration = Duration::from_millis(200);
const DEBOUNCE: Duration = Duration::from_millis(150);

enum Event {
    Request(PathBuf),
    WatcherFired,
    Eof,
}

fn main() {
    let mut fish_pid: Option<i32> = None;
    let mut status_file: Option<PathBuf> = None;
    let mut request_fifo: Option<PathBuf> = None;
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

    spawn_watchdog();

    let (tx, rx) = mpsc::channel();

    spawn_fifo_reader(request_fifo, tx.clone());

    let watch_tx = tx.clone();
    let mut debouncer: Debouncer<notify::RecommendedWatcher, FileIdMap> =
        new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            // We don't care which path fired — main thread re-computes for the
            // current PWD. (See the path-match guard in fish_prompt for why
            // this is safe even if a non-current repo's events trigger us.)
            if let Ok(events) = result {
                if !events.is_empty() {
                    watch_tx.send(Event::WatcherFired).ok();
                }
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

    while let Ok(event) = rx.recv() {
        match event {
            Event::Eof => break,
            Event::Request(path) => {
                current_pwd = Some(path.clone());
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
                    }
                    if let Some(ref new) = new_git_dir {
                        if watch_repo(&mut debouncer, new).is_ok() {
                            watched_git_dir = Some(new.clone());
                        }
                    }
                }

                let status = repo
                    .as_ref()
                    .and_then(|r| compute_status_for_repo(r, dirty_deadline));
                let _ = write_status_file(&status_file, &path, status.as_ref());
                signal_fish(fish_pid);
            }
            Event::WatcherFired => {
                let Some(pwd) = current_pwd.as_deref() else {
                    continue;
                };
                let status = gix::discover(pwd)
                    .ok()
                    .and_then(|r| compute_status_for_repo(&r, dirty_deadline));
                let _ = write_status_file(&status_file, pwd, status.as_ref());
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
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        // When fish dies, this process gets reparented to init/launchd, so
        // getppid() returns a different value than at startup.
        let current = unsafe { libc::getppid() };
        if current != initial_ppid {
            std::process::exit(0);
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
    fn is_full(&self) -> bool {
        self.staged && self.modified && self.untracked && self.conflict
    }
    fn any(&self) -> bool {
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

fn compute_status_for_repo(repo: &gix::Repository, dirty_deadline: Duration) -> Option<Status> {
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
        .map(|n| compute_upstream(repo, n.as_ref()))
        .unwrap_or(UpstreamState::None);

    Some(Status {
        branch,
        upstream,
        dirty: compute_dirty(repo, dirty_deadline),
        operation: detect_operation(repo.git_dir()),
        stash: count_stashes(repo.git_dir()),
    })
}

// Counts entries in the stash reflog. Each `git stash push` appends one line;
// `stash pop`/`drop` rewrites the file with one fewer line. Absent file means
// zero stashes (the normal case for repos that have never been stashed).
fn count_stashes(git_dir: &Path) -> u32 {
    match std::fs::read_to_string(git_dir.join("logs/refs/stash")) {
        Ok(content) => content.lines().count() as u32,
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
    let tracking =
        match repo.branch_remote_tracking_ref_name(head_name, gix::remote::Direction::Fetch) {
            Some(Ok(t)) => t,
            _ => return UpstreamState::None,
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
            .map(|walk| walk.filter_map(Result::ok).count() as u32)
            .unwrap_or(0)
    };

    UpstreamState::Tracking {
        ahead: count_walk(head_id, upstream_id),
        behind: count_walk(upstream_id, head_id),
    }
}

// Iterates gix's parallel status engine, classifying each item into one of
// four flags (staged / modified / untracked / conflict). Short-circuits once
// all four are observed; otherwise drains until the deadline expires.
//
// Returns:
//   - Dirty(flags) when at least one flag was observed
//   - Unknown when no items were seen and the deadline expired
//   - Clean when the iterator drained without any items
fn compute_dirty(repo: &gix::Repository, deadline: Duration) -> DirtyState {
    let interrupt = Arc::new(AtomicBool::new(false));
    {
        let interrupt = interrupt.clone();
        std::thread::spawn(move || {
            std::thread::sleep(deadline);
            interrupt.store(true, Ordering::SeqCst);
        });
    }

    let platform = match repo.status(gix::progress::Discard) {
        Ok(p) => p.should_interrupt_owned(interrupt.clone()),
        Err(_) => return DirtyState::Unknown,
    };

    let iter = match platform.into_iter(None) {
        Ok(it) => it,
        Err(_) => return DirtyState::Unknown,
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
    } else if interrupt.load(Ordering::SeqCst) {
        DirtyState::Unknown
    } else {
        DirtyState::Clean
    }
}

fn classify_item(item: gix::status::Item, flags: &mut DirtyFlags) {
    use gix::status::index_worktree::Item as IWItem;
    use gix::status::plumbing::index_as_worktree::EntryStatus;
    use gix::status::Item;

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
