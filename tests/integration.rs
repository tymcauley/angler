use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Once;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const DAEMON: &str = env!("CARGO_BIN_EXE_angler-daemon");
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

// SIGUSR1's default action is terminate. The daemon signals our test process,
// so we must ignore it before any test runs.
fn ignore_sigusr1() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        libc::signal(libc::SIGUSR1, libc::SIG_IGN);
    });
}

struct Harness {
    _state: TempDir,
    request_fifo: PathBuf,
    status_file: PathBuf,
    daemon: Child,
}

impl Harness {
    fn new() -> Self {
        Self::with_args(&[])
    }

    fn with_args(extra: &[&str]) -> Self {
        ignore_sigusr1();

        let state = tempfile::tempdir().expect("mkdtemp");
        let request_fifo = state.path().join("req");
        let status_file = state.path().join("status");

        // Use mkfifo via libc (avoid shelling out for a single syscall).
        let c_path = std::ffi::CString::new(request_fifo.as_os_str().as_encoded_bytes()).unwrap();
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed");

        let pid = i32::try_from(std::process::id()).unwrap();
        let mut cmd = Command::new(DAEMON);
        cmd.args([
            "--fish-pid",
            &pid.to_string(),
            "--state-dir",
            state.path().to_str().unwrap(),
        ]);
        cmd.args(extra);
        // Hermetic from the test runner's user git config — without this,
        // settings like diff.ignoreSubmodules bleed in and silently change
        // what gix reports.
        cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        let daemon = cmd.spawn().expect("spawn daemon");

        Self {
            _state: state,
            request_fifo,
            status_file,
            daemon,
        }
    }

    fn request(&self, path: &Path) {
        // The daemon opens the FIFO with O_RDWR, so write-only open should not
        // block. Retry briefly to absorb the spawn race.
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut writer = loop {
            match OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.request_fifo)
            {
                Ok(f) => break f,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("could not open request FIFO for write: {e}"),
            }
        };
        // Wire framing: `AN1\0<path>\0`. NUL-terminated keeps non-UTF-8 path
        // bytes intact and matches the daemon's reader.
        writer.write_all(b"AN1\0").expect("write version");
        writer
            .write_all(path.as_os_str().as_bytes())
            .expect("write request");
        writer.write_all(&[0]).expect("write terminator");
    }

    fn wait_for(&self, expected_path: &Path) -> Fields {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            if let Ok(fields) = read_status(&self.status_file)
                && fields.path.as_os_str() == expected_path.as_os_str()
            {
                return fields;
            }
            assert!(
                Instant::now() < deadline,
                "daemon did not respond for {} within {:?}",
                expected_path.display(),
                RESPONSE_TIMEOUT
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // Wait until predicate(fields) is true. Used for fsmonitor tests where
    // the daemon spontaneously re-emits without a fresh FIFO request, so
    // wait_for() (which keys on path) doesn't apply.
    fn wait_until(
        &self,
        timeout: Duration,
        mut predicate: impl FnMut(&Fields) -> bool,
    ) -> Option<Fields> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(fields) = read_status(&self.status_file)
                && predicate(&fields)
            {
                return Some(fields);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

#[derive(Debug)]
struct Fields {
    path: PathBuf,
    branch: String,
    ahead: String,
    behind: String,
    dirty: String,
    operation: String,
    upstream: String,
    stash: String,
    submodules: String,
}

fn read_status(p: &Path) -> std::io::Result<Fields> {
    let mut buf = Vec::new();
    File::open(p)?.read_to_end(&mut buf)?;
    let parts: Vec<&[u8]> = buf.split(|&b| b == 0).collect();
    // Wire framing: `AN1\0` sentinel + 9 payload fields. Reject anything
    // else — the daemon never writes a different shape, so a mismatch
    // means a partial write or a version skew, not a soft fallback.
    if parts.first().copied() != Some(b"AN1".as_slice()) {
        return Err(std::io::Error::other("missing AN1 wire-version sentinel"));
    }
    if parts.len() < 10 {
        return Err(std::io::Error::other("fewer than 9 payload fields"));
    }
    // Path can contain non-UTF-8 bytes — keep them. Other fields are ASCII
    // by daemon contract, so lossy UTF-8 is fine.
    let s = |b: &[u8]| std::str::from_utf8(b).unwrap_or("").to_owned();
    Ok(Fields {
        path: PathBuf::from(std::ffi::OsStr::from_bytes(parts[1])),
        branch: s(parts[2]),
        ahead: s(parts[3]),
        behind: s(parts[4]),
        dirty: s(parts[5]),
        operation: s(parts[6]),
        upstream: s(parts[7]),
        stash: s(parts[8]),
        submodules: s(parts[9]),
    })
}

// ---- fixtures ----

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        // Empty config so user's commit hooks/templates can't bleed in.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .expect("git invocation failed");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr),
    );
}

fn make_clean_repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), b"hello\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    dir
}

fn rev_parse(repo: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

// Build a repo with branch.main.* upstream config but NO refs/remotes/origin/main —
// the gone-upstream case. Mirrors what happens after a remote branch is deleted
// (e.g. squash-merge cleanup) and the local tracking ref is pruned.
fn make_repo_with_gone_upstream() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("a.txt"), b"hi\n").unwrap();
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(dir.path(), &["config", "branch.main.remote", "origin"]);
    git(
        dir.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    git(
        dir.path(),
        &[
            "config",
            "remote.origin.url",
            "git@example.invalid:placeholder.git",
        ],
    );
    git(
        dir.path(),
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    // Crucially: do NOT create refs/remotes/origin/main.
    dir
}

// Build a repo where main has `ahead` commits past the synthetic upstream and
// the upstream has `behind` commits past main's fork point. No real remote is
// involved — we fabricate refs/remotes/origin/main directly via update-ref and
// configure branch.main.{remote,merge} so gix treats it as the upstream.
fn make_repo_with_upstream(ahead: u32, behind: u32) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("base.txt"), b"base\n").unwrap();
    git(dir.path(), &["add", "base.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "base"]);
    let fork_point = rev_parse(dir.path(), "HEAD");

    // Build the upstream side first (so we can capture its tip), then reset
    // local main back to the fork point and build the local-only commits.
    for i in 0..behind {
        let name = format!("upstream-{i}.txt");
        std::fs::write(dir.path().join(&name), b"x\n").unwrap();
        git(dir.path(), &["add", &name]);
        git(
            dir.path(),
            &["commit", "-q", "-m", &format!("upstream {i}")],
        );
    }
    let upstream_tip = rev_parse(dir.path(), "HEAD");

    git(
        dir.path(),
        &["update-ref", "refs/remotes/origin/main", &upstream_tip],
    );
    git(dir.path(), &["reset", "-q", "--hard", &fork_point]);

    for i in 0..ahead {
        let name = format!("local-{i}.txt");
        std::fs::write(dir.path().join(&name), b"x\n").unwrap();
        git(dir.path(), &["add", &name]);
        git(dir.path(), &["commit", "-q", "-m", &format!("local {i}")]);
    }

    git(dir.path(), &["config", "branch.main.remote", "origin"]);
    git(
        dir.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    // gix's branch_remote_tracking_ref_name resolves the remote in full, which
    // requires a URL even though we never fetch anything.
    git(
        dir.path(),
        &[
            "config",
            "remote.origin.url",
            "git@example.invalid:placeholder.git",
        ],
    );
    git(
        dir.path(),
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );

    dir
}

fn make_dirty_repo() -> TempDir {
    let dir = make_clean_repo();
    std::fs::write(dir.path().join("a.txt"), b"changed\n").unwrap();
    dir
}

fn make_detached_head_repo() -> TempDir {
    let dir = make_clean_repo();
    let out = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let sha = String::from_utf8(out.stdout).unwrap();
    let sha = sha.trim();
    git(
        dir.path(),
        &["-c", "advice.detachedHead=false", "checkout", sha],
    );
    dir
}

fn no_repo_dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

// Build a parent repo containing a submodule whose worktree has been
// dirtied. The parent itself has no other uncommitted changes — the only
// possible dirty signal is the submodule. Returns both temp dirs so callers
// keep them alive for the lifetime of the test.
fn make_repo_with_dirty_submodule() -> (TempDir, TempDir) {
    let sub = tempfile::tempdir().unwrap();
    git(sub.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(sub.path().join("a.txt"), b"sub\n").unwrap();
    git(sub.path(), &["add", "a.txt"]);
    git(sub.path(), &["commit", "-q", "-m", "init"]);

    let parent = tempfile::tempdir().unwrap();
    git(parent.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(parent.path().join("a.txt"), b"parent\n").unwrap();
    git(parent.path(), &["add", "a.txt"]);
    git(parent.path(), &["commit", "-q", "-m", "init"]);

    // Local file:// URLs require protocol.file.allow=always on modern git.
    git(
        parent.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub.path().to_str().unwrap(),
            "sub",
        ],
    );
    git(parent.path(), &["commit", "-q", "-m", "add submodule"]);

    // Modify a tracked file inside the submodule's worktree.
    std::fs::write(parent.path().join("sub/a.txt"), b"changed\n").unwrap();

    (parent, sub)
}

fn make_repo_with_stashes(n: u32) -> TempDir {
    let dir = make_clean_repo();
    for i in 0..n {
        // git stash push needs a real change to stash; the stash itself
        // resets the working tree, so we re-write each iteration.
        std::fs::write(dir.path().join("a.txt"), format!("change {i}\n").as_bytes()).unwrap();
        git(
            dir.path(),
            &["stash", "push", "-q", "-m", &format!("change {i}")],
        );
    }
    dir
}

// ---- tests ----

#[test]
fn clean_repo_is_clean_with_branch_main() {
    let h = Harness::new();
    let repo = make_clean_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.branch, "main");
    assert_eq!(f.dirty, "0");
    // ahead/behind are stubbed to 0 in commit-1; this asserts the wire format.
    assert_eq!(f.ahead, "0");
    assert_eq!(f.behind, "0");
}

#[test]
fn dirty_repo_reports_dirty() {
    let h = Harness::new();
    let repo = make_dirty_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.branch, "main");
    // Modified tracked file → 'modified' flag only.
    assert_eq!(f.dirty, "*");
}

#[test]
fn untracked_file_shows_untracked_flag() {
    let h = Harness::new();
    let repo = make_clean_repo();
    std::fs::write(repo.path().join("new.txt"), b"hello\n").unwrap();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.dirty, "u");
}

#[test]
fn staged_change_shows_staged_flag() {
    let h = Harness::new();
    let repo = make_clean_repo();
    std::fs::write(repo.path().join("a.txt"), b"changed\n").unwrap();
    git(repo.path(), &["add", "a.txt"]);
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.dirty, "+");
}

#[test]
fn staged_plus_modified_shows_both_flags() {
    let h = Harness::new();
    let repo = make_clean_repo();
    std::fs::write(repo.path().join("a.txt"), b"first edit\n").unwrap();
    git(repo.path(), &["add", "a.txt"]);
    // Now modify on top of the staged version → both staged and modified.
    std::fs::write(repo.path().join("a.txt"), b"second edit\n").unwrap();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    // Order in the wire encoding is staged then modified.
    assert_eq!(f.dirty, "+*");
}

#[test]
fn non_repo_path_has_empty_git_fields() {
    let h = Harness::new();
    let dir = no_repo_dir();
    h.request(dir.path());
    let f = h.wait_for(dir.path());
    assert_eq!(f.branch, "");
    assert_eq!(f.dirty, "");
}

#[test]
fn deferred_dirty_resolves_after_deadline() {
    // 0 ms deadline: recv_timeout fires immediately, so the initial response
    // goes out as Unknown via the deferred path. The background worker keeps
    // running and eventually delivers the real result via the main channel;
    // the daemon re-emits and the status file flips to "*".
    //
    // We deliberately don't assert on the intermediate "?" state — on fast
    // machines the deferred resolution can land before our polling reader
    // observes the initial write. The deferred path's correctness is implied
    // by the eventual "*": if deferral were broken, the file would stay at
    // "?" and wait_until would time out.
    let h = Harness::with_args(&["--dirty-deadline-ms", "0"]);
    let repo = make_dirty_repo();
    h.request(repo.path());

    let resolved = h
        .wait_until(Duration::from_secs(2), |f| f.dirty == "*")
        .expect("deferred dirty should eventually resolve to '*'");
    assert_eq!(resolved.path, repo.path());
    assert_eq!(resolved.branch, "main");
}

#[test]
fn detached_head_reports_short_sha() {
    let h = Harness::new();
    let repo = make_detached_head_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    // Expect a 7-char hex SHA, not "main".
    assert_eq!(
        f.branch.len(),
        7,
        "expected 7-char short sha, got {:?}",
        f.branch
    );
    assert!(
        f.branch.chars().all(|c| c.is_ascii_hexdigit()),
        "expected hex sha, got {:?}",
        f.branch,
    );
}

#[test]
fn ahead_only_reports_correct_count() {
    let h = Harness::new();
    let repo = make_repo_with_upstream(3, 0);
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.ahead, "3");
    assert_eq!(f.behind, "0");
}

#[test]
fn behind_only_reports_correct_count() {
    let h = Harness::new();
    let repo = make_repo_with_upstream(0, 2);
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.ahead, "0");
    assert_eq!(f.behind, "2");
}

#[test]
fn diverged_reports_both_counts() {
    let h = Harness::new();
    let repo = make_repo_with_upstream(2, 4);
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.ahead, "2");
    assert_eq!(f.behind, "4");
}

#[test]
fn no_upstream_reports_zero() {
    let h = Harness::new();
    // make_clean_repo has no remote configured.
    let repo = make_clean_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.ahead, "0");
    assert_eq!(f.behind, "0");
}

#[test]
fn detached_head_reports_zero_ahead_behind() {
    let h = Harness::new();
    let repo = make_detached_head_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.ahead, "0");
    assert_eq!(f.behind, "0");
}

#[test]
fn stash_count_is_zero_for_clean_repo() {
    let h = Harness::new();
    let repo = make_clean_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.stash, "0");
}

#[test]
fn stash_count_reflects_pushed_stashes() {
    let h = Harness::new();
    let repo = make_repo_with_stashes(3);
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.stash, "3");
}

#[test]
fn gone_upstream_is_reported() {
    let h = Harness::new();
    let repo = make_repo_with_gone_upstream();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.upstream, "gone");
    assert_eq!(f.ahead, "0");
    assert_eq!(f.behind, "0");
}

#[test]
fn no_upstream_is_not_reported_as_gone() {
    let h = Harness::new();
    let repo = make_clean_repo(); // no branch.*.remote configured
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.upstream, "");
}

#[test]
fn tracking_upstream_is_not_reported_as_gone() {
    let h = Harness::new();
    let repo = make_repo_with_upstream(2, 1);
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.upstream, "");
    assert_eq!(f.ahead, "2");
    assert_eq!(f.behind, "1");
}

#[test]
fn detects_in_progress_operations() {
    // Each entry: (file or dir to create under .git, expected operation label).
    // We test by hand-creating the marker since simulating a real conflicted
    // rebase/merge requires more fixture machinery than the marker check.
    let cases: &[(&str, &str)] = &[
        ("MERGE_HEAD", "merging"),
        ("CHERRY_PICK_HEAD", "cherry-picking"),
        ("REVERT_HEAD", "reverting"),
        ("BISECT_LOG", "bisecting"),
    ];

    for (marker, label) in cases {
        let h = Harness::new();
        let repo = make_clean_repo();
        std::fs::write(repo.path().join(".git").join(marker), b"placeholder").unwrap();
        h.request(repo.path());
        let f = h.wait_for(repo.path());
        assert_eq!(f.operation, *label, "marker {marker}");
    }
}

#[test]
fn rebase_directory_reports_rebasing() {
    let h = Harness::new();
    let repo = make_clean_repo();
    std::fs::create_dir(repo.path().join(".git").join("rebase-merge")).unwrap();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.operation, "rebasing");
}

#[test]
fn rebase_wins_over_merge_when_both_present() {
    let h = Harness::new();
    let repo = make_clean_repo();
    std::fs::create_dir(repo.path().join(".git").join("rebase-merge")).unwrap();
    std::fs::write(repo.path().join(".git").join("MERGE_HEAD"), b"x").unwrap();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    // During a rebase that hits a conflict, both markers can exist; user
    // thinks of themselves as rebasing, not merging.
    assert_eq!(f.operation, "rebasing");
}

#[test]
fn no_operation_when_idle() {
    let h = Harness::new();
    let repo = make_clean_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.operation, "");
}

#[test]
fn fsmonitor_picks_up_branch_change_without_new_request() {
    let h = Harness::new();
    let repo = make_clean_repo();

    // Initial request — daemon also starts watching this repo.
    h.request(repo.path());
    let initial = h.wait_for(repo.path());
    assert_eq!(initial.branch, "main");

    // Switch branches in the worktree directly, without telling the daemon.
    // This writes .git/HEAD and creates .git/refs/heads/feature, both in the
    // daemon's watch set. We expect the daemon to re-emit on its own.
    git(repo.path(), &["checkout", "-q", "-b", "feature"]);

    // Generous budget: macOS FSEvents has its own coalescing latency on top
    // of our 150ms debounce, plus the recompute time.
    let updated = h
        .wait_until(Duration::from_secs(5), |f| f.branch == "feature")
        .expect("daemon should have observed the branch change via fsmonitor");
    assert_eq!(updated.path, repo.path());
    assert_eq!(updated.branch, "feature");
}

#[test]
fn log_file_records_lifecycle_events() {
    // Spawn with --log-file and a simple request flow; verify the resulting
    // file contains the major events (start, request, status) and that each
    // line is prefixed with the daemon's PID.
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("daemon.log");
    let h = Harness::with_args(&["--log-file", log_path.to_str().unwrap()]);
    let repo = make_clean_repo();
    h.request(repo.path());
    let _ = h.wait_for(repo.path());

    // Logger writes happen on the same thread as the event handler, but the
    // OS may not flush immediately. A short poll absorbs that.
    let deadline = Instant::now() + Duration::from_secs(1);
    let contents = loop {
        if let Ok(s) = std::fs::read_to_string(&log_path)
            && s.contains("status ")
        {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "log file never grew to include 'status': {log_path:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    assert!(
        contents.contains("start "),
        "expected 'start' event in log, got:\n{contents}",
    );
    assert!(
        contents.contains("request "),
        "expected 'request' event in log, got:\n{contents}",
    );
    assert!(
        contents.contains("branch=main"),
        "expected status entry to include the branch, got:\n{contents}",
    );

    // Every line must have the form `<rfc3339> [<pid>] <message>`. Spot-check
    // one non-empty line.
    let first_line = contents.lines().find(|l| !l.is_empty()).unwrap();
    assert!(
        first_line.contains(" ["),
        "log line missing PID bracket: {first_line:?}",
    );
}

#[test]
fn no_log_file_created_when_flag_omitted() {
    // The harness's tempdir holds the FIFO and status file; no daemon log
    // should appear there (or anywhere we control) when --log-file isn't
    // passed.
    let h = Harness::new();
    let repo = make_clean_repo();
    h.request(repo.path());
    let _ = h.wait_for(repo.path());

    let names: Vec<String> = std::fs::read_dir(h._state.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(
        !names.iter().any(|n| n.ends_with(".log")),
        "no .log file expected in harness dir, got {names:?}",
    );
}

#[test]
fn repeated_identical_requests_skip_after_first_write() {
    // fish_prompt fires a request on every render. Without idempotency,
    // each request produces a SIGUSR1, which triggers commandline -f
    // repaint, which fires another fish_prompt, which fires another
    // request — an infinite loop. The daemon must skip writes (and the
    // signal) when the new bytes match the last write.
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("daemon.log");
    let h = Harness::with_args(&["--log-file", log_path.to_str().unwrap()]);
    let repo = make_clean_repo();

    h.request(repo.path());
    let _ = h.wait_for(repo.path());

    // Subsequent identical requests should be skipped at the daemon.
    for _ in 0..5 {
        h.request(repo.path());
    }

    // Wait for the daemon to finish processing all 6 requests. Each one
    // produces either a status_skip (idempotent re-walk landed on same
    // bytes) or a walk_coalesced (a walk was already in flight at request
    // arrival, so this one didn't kick a new walk). Which one wins depends
    // on whether the previous WalkComplete was processed before the next
    // Request — that's a race we don't try to control here. We just want
    // the total to add up and the count of real writes to stay at one.
    let deadline = Instant::now() + Duration::from_secs(2);
    let contents = loop {
        if let Ok(s) = std::fs::read_to_string(&log_path)
            && s.matches("status_skip").count() + s.matches("walk_coalesced").count() >= 5
        {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "expected ≥5 follow-up events; log was:\n{}",
            std::fs::read_to_string(&log_path).unwrap_or_default(),
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    // Exactly one "status branch=…" write — the first request. Every later
    // request falls through to status_skip or walk_coalesced.
    let writes = contents.matches("] status branch=").count();
    assert_eq!(
        writes, 1,
        "expected exactly 1 status write, got {writes}; log:\n{contents}",
    );
}

#[test]
fn bursty_requests_coalesce_into_few_walks() {
    // Send a burst of 20 identical requests. With coalescing we expect
    // many fewer walks than 20 — only the first request kicks one, and
    // each subsequent walk only fires if a request arrived while the
    // previous walk was running (and pending kicks fold many requests
    // into one walk). The exact count is timing-dependent (faster walks
    // mean fewer coalesces), but it must be strictly less than the
    // request count for any plausible CI machine.
    let log_dir = tempfile::tempdir().unwrap();
    let log_path = log_dir.path().join("daemon.log");
    let h = Harness::with_args(&["--log-file", log_path.to_str().unwrap()]);
    let repo = make_clean_repo();

    h.request(repo.path());
    let _ = h.wait_for(repo.path());

    const N: usize = 20;
    for _ in 0..N {
        h.request(repo.path());
    }

    // Wait for the burst to be fully processed. We're done when the
    // count of (status_skip + walk_coalesced) entries from the burst
    // covers all N follow-up requests.
    let deadline = Instant::now() + Duration::from_secs(3);
    let contents = loop {
        if let Ok(s) = std::fs::read_to_string(&log_path)
            && s.matches("status_skip").count() + s.matches("walk_coalesced").count() >= N
        {
            break s;
        }
        assert!(
            Instant::now() < deadline,
            "burst didn't settle; log:\n{}",
            std::fs::read_to_string(&log_path).unwrap_or_default(),
        );
        std::thread::sleep(Duration::from_millis(20));
    };

    let walks = contents.matches("dirty_walk dur_ms=").count();
    let coalesced = contents.matches("walk_coalesced").count();

    // 21 requests total (1 setup + 20 burst). Without coalescing we'd
    // see 21 walks. With coalescing we expect substantially fewer.
    assert!(
        walks < N + 1,
        "expected < {} walks for {} requests; got walks={walks} coalesced={coalesced}; log:\n{contents}",
        N + 1,
        N + 1,
    );
    // And at least one request must have actually been coalesced — if
    // every request raced ahead of WalkComplete, walks would equal
    // requests and the assertion above would fail; this one ensures the
    // coalesce path itself was exercised.
    assert!(
        coalesced >= 1,
        "expected ≥1 walk_coalesced; got {coalesced}; log:\n{contents}",
    );
}

#[test]
fn submodule_changes_surface_separately_from_dirty() {
    // The parent has no uncommitted changes of its own; only the submodule
    // worktree is dirty. We expect the parent's `dirty` to stay clean
    // (submodule changes don't bleed into `*`) while `submodules` reports
    // the count.
    let h = Harness::new();
    let (parent, _sub) = make_repo_with_dirty_submodule();
    h.request(parent.path());
    let f = h.wait_for(parent.path());
    assert_eq!(f.dirty, "0");
    assert_eq!(f.submodules, "1");
}

#[test]
fn no_submodules_reports_zero() {
    let h = Harness::new();
    let repo = make_clean_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.submodules, "0");
}

#[test]
fn rapid_cd_converges_on_final_path() {
    // cd a; cd b; cd c with no waits between. Coalescing means walks for
    // a/b can be in flight or queued when c arrives; the generation guard
    // drops their results, and the pending-kick after a busy WalkComplete
    // re-fires the walk against `current_pwd` (c) rather than the original
    // requestor's path. Verifies all that converges on c eventually.
    let h = Harness::new();
    let a = make_clean_repo();
    let b = make_clean_repo();
    let c = make_clean_repo();

    h.request(a.path());
    h.request(b.path());
    h.request(c.path());

    let f = h.wait_for(c.path());
    assert_eq!(f.branch, "main");
}

#[test]
fn watched_repo_deleted_does_not_break_daemon() {
    // Daemon is watching repo A's .git/. We rm -rf A while the daemon is
    // running, which fires watcher events on now-missing paths. The daemon
    // must not crash or wedge — proven by responding to a subsequent
    // request for a different repo B.
    let h = Harness::new();
    let repo = make_clean_repo();
    let repo_path = repo.path().to_path_buf();
    h.request(&repo_path);
    let _ = h.wait_for(&repo_path);

    // Recursive delete via tempdir's Drop. The daemon's watch on the now-
    // gone .git/ produces FSEvents/inotify events; our debouncer batches
    // them and forwards a WatcherFired which the main loop handles by
    // re-walking the (now-vanished) pwd.
    drop(repo);

    // Wait long enough for the deletion events to debounce and flow
    // through the main loop. DEBOUNCE is 150ms; 300ms gives margin and
    // also ensures any panic-driving codepath has fired before our next
    // request arrives.
    std::thread::sleep(Duration::from_millis(300));

    // Daemon must still be alive and processing. Use a fresh repo to
    // also exercise swap_repo_watch's behavior when the previously-
    // watched git_dir no longer exists on disk.
    let other = make_clean_repo();
    h.request(other.path());
    let f = h.wait_for(other.path());
    assert_eq!(f.branch, "main");
}

#[test]
fn fifo_handles_path_with_non_utf8_bytes() {
    // NUL-delimited framing means non-UTF-8 path bytes survive the FIFO
    // round-trip. With newline-delimited String reads, lines() would error
    // on the 0xff byte and tear down the reader thread.
    let h = Harness::new();
    let mut bytes = b"/tmp/".to_vec();
    bytes.extend_from_slice(&[0xff, 0xfe]);
    bytes.extend_from_slice(b"weird-path");
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
    h.request(&path);
    let f = h.wait_for(&path);
    // The path doesn't exist; daemon reports it as a non-repo. The point of
    // this test is that the daemon responded at all — i.e., the reader
    // didn't choke on the non-UTF-8 bytes.
    assert_eq!(f.branch, "");
}

#[test]
fn fifo_handles_path_with_embedded_newline() {
    // Embedded newlines are legal in Unix paths. NUL-delimited framing
    // round-trips them; newline-delimited would split the request in two.
    let h = Harness::new();
    let path = PathBuf::from("/tmp/has\nnewline");
    h.request(&path);
    let f = h.wait_for(&path);
    assert_eq!(f.branch, "");
}

#[test]
fn multiple_requests_in_sequence() {
    let h = Harness::new();
    let clean = make_clean_repo();
    let dirty = make_dirty_repo();
    let none = no_repo_dir();

    h.request(clean.path());
    let f = h.wait_for(clean.path());
    assert_eq!(f.dirty, "0");

    h.request(dirty.path());
    let f = h.wait_for(dirty.path());
    assert_eq!(f.dirty, "*");

    h.request(none.path());
    let f = h.wait_for(none.path());
    assert_eq!(f.branch, "");
}

#[test]
fn daemon_exits_and_cleans_up_when_parent_dies() {
    // Use sh as an intermediate parent so we can kill it without taking
    // down the test runner. sh backgrounds the daemon, prints its PID on
    // stdout, then sleeps; killing sh reparents the daemon, and its
    // PR_SET_PDEATHSIG / kqueue arming fires.
    let state = tempfile::tempdir().expect("mkdtemp");
    let request_fifo = state.path().join("req");
    let c_path = std::ffi::CString::new(request_fifo.as_os_str().as_encoded_bytes()).unwrap();
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo failed");

    // fish_pid is just the SIGUSR1 destination. Test runner ignores SIGUSR1
    // (via ignore_sigusr1 in other tests), but this test doesn't trigger any
    // walk, so there should be no signal traffic. Use the test runner's
    // PID anyway so the daemon has a valid target.
    let test_pid = std::process::id();
    let mut shell = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{} --fish-pid {} --state-dir {} >/dev/null 2>&1 & echo $! && sleep 60",
            DAEMON,
            test_pid,
            state.path().display(),
        ))
        .stdout(Stdio::piped())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .spawn()
        .expect("spawn sh");

    let mut buf = String::new();
    BufReader::new(shell.stdout.take().unwrap())
        .read_line(&mut buf)
        .expect("read daemon pid from sh");
    let daemon_pid: i32 = buf.trim().parse().expect("parse daemon pid");

    // Give the daemon a moment to arm parent-death detection. Without
    // this, the kill below could land in the registration race window;
    // the daemon handles that case (re-checks getppid / kill -0), but the
    // test wants to exercise the steady-state path.
    std::thread::sleep(Duration::from_millis(200));

    // Kill sh — daemon's parent task is now gone.
    shell.kill().expect("kill sh");
    shell.wait().expect("reap sh");

    // Daemon should exit within ~1s.
    let deadline = Instant::now() + Duration::from_secs(1);
    while unsafe { libc::kill(daemon_pid, 0) } == 0 {
        assert!(
            Instant::now() < deadline,
            "daemon (pid={daemon_pid}) did not exit within 1s after parent died"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // And state_dir should be cleaned up.
    let cleanup_deadline = Instant::now() + Duration::from_millis(200);
    while state.path().exists() {
        assert!(
            Instant::now() < cleanup_deadline,
            "state_dir {} still present after daemon exit",
            state.path().display(),
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
