use std::collections::VecDeque;
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
const MAX_WATCHED_REPOS: usize = 16;

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

    // git_dir paths in least-recently-touched order; back is most recent.
    let mut watched: VecDeque<PathBuf> = VecDeque::new();
    // The current PWD fish has told us about. We render and re-render against
    // this; watcher fires that don't pertain to it produce no visible effect
    // because of fish_prompt's path-match guard.
    let mut current_pwd: Option<PathBuf> = None;

    while let Ok(event) = rx.recv() {
        match event {
            Event::Eof => break,
            Event::Request(path) => {
                current_pwd = Some(path.clone());
                let repo = gix::discover(&path).ok();

                let status = repo
                    .as_ref()
                    .and_then(|r| compute_status_for_repo(r, dirty_deadline));
                let _ = write_status_file(&status_file, &path, status.as_ref());
                signal_fish(fish_pid);

                if let Some(repo) = repo {
                    let git_dir = repo.git_dir().to_path_buf();
                    touch_or_add(&mut watched, &mut debouncer, git_dir);
                }
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

// Move repo to back of LRU; if new, register watches and evict oldest if over
// the cap.
fn touch_or_add(
    watched: &mut VecDeque<PathBuf>,
    debouncer: &mut Debouncer<notify::RecommendedWatcher, FileIdMap>,
    git_dir: PathBuf,
) {
    if let Some(pos) = watched.iter().position(|p| p == &git_dir) {
        watched.remove(pos);
        watched.push_back(git_dir);
        return;
    }
    if watch_repo(debouncer, &git_dir).is_err() {
        return;
    }
    watched.push_back(git_dir);
    while watched.len() > MAX_WATCHED_REPOS {
        if let Some(oldest) = watched.pop_front() {
            let _ = unwatch_repo(debouncer, &oldest);
        }
    }
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

fn unwatch_repo(
    debouncer: &mut Debouncer<notify::RecommendedWatcher, FileIdMap>,
    git_dir: &Path,
) -> Result<(), notify::Error> {
    let _ = debouncer.unwatch(git_dir);
    let _ = debouncer.unwatch(git_dir.join("refs"));
    Ok(())
}

struct Status {
    branch: String,
    ahead: u32,
    behind: u32,
    dirty: DirtyState,
}

#[derive(Copy, Clone)]
enum DirtyState {
    Clean,
    Dirty,
    Unknown,
}

impl DirtyState {
    fn as_byte(self) -> &'static str {
        match self {
            DirtyState::Clean => "0",
            DirtyState::Dirty => "1",
            DirtyState::Unknown => "?",
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

    let (ahead, behind) = head_name
        .as_ref()
        .and_then(|n| compute_ahead_behind(repo, n.as_ref()))
        .unwrap_or((0, 0));

    Some(Status {
        branch,
        ahead,
        behind,
        dirty: compute_dirty(repo, dirty_deadline),
    })
}

// Returns (ahead, behind) for `head_name` against its configured upstream
// tracking branch. Returns None on any failure (no upstream, gone upstream,
// missing refs, etc.) so the caller can fall through to (0, 0).
fn compute_ahead_behind(
    repo: &gix::Repository,
    head_name: &gix::refs::FullNameRef,
) -> Option<(u32, u32)> {
    let tracking = repo
        .branch_remote_tracking_ref_name(head_name, gix::remote::Direction::Fetch)?
        .ok()?;

    let head_id = repo.head_id().ok()?.detach();
    let upstream_id = repo
        .find_reference(tracking.as_ref())
        .ok()?
        .peel_to_id()
        .ok()?
        .detach();

    let count_walk = |from: gix::ObjectId, hide: gix::ObjectId| -> Option<u32> {
        let walk = repo.rev_walk([from]).with_hidden([hide]).all().ok()?;
        Some(walk.filter_map(Result::ok).count() as u32)
    };

    Some((
        count_walk(head_id, upstream_id).unwrap_or(0),
        count_walk(upstream_id, head_id).unwrap_or(0),
    ))
}

// Iterates gix's parallel status engine until either:
//   - an item is yielded (any change → Dirty, short-circuit)
//   - the iterator drains naturally (Clean)
//   - the deadline expires (Unknown — unless we already saw a change)
//
// The deadline is enforced via a timer thread that flips the AtomicBool
// gix's workers periodically check.
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

    let mut found_change = false;
    for item in iter {
        if item.is_ok() {
            found_change = true;
            break;
        }
    }

    match (found_change, interrupt.load(Ordering::SeqCst)) {
        (true, _) => DirtyState::Dirty,
        (false, true) => DirtyState::Unknown,
        (false, false) => DirtyState::Clean,
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
        // Each field NUL-terminated. 5 fields:
        //   request_path, branch, ahead, behind, dirty
        // For non-repos, the last 4 fields are empty.
        f.write_all(request_path.as_os_str().as_bytes())?;
        f.write_all(b"\0")?;
        if let Some(s) = status {
            write!(
                f,
                "{}\0{}\0{}\0{}\0",
                s.branch,
                s.ahead,
                s.behind,
                s.dirty.as_byte()
            )?;
        } else {
            f.write_all(b"\0\0\0\0")?;
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}
