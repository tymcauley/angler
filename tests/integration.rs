use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Once;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const DAEMON: &str = env!("CARGO_BIN_EXE_fish-prompt-daemon");
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

        let pid = std::process::id() as i32;
        let mut cmd = Command::new(DAEMON);
        cmd.args([
            "--fish-pid",
            &pid.to_string(),
            "--status-file",
            status_file.to_str().unwrap(),
            "--request-fifo",
            request_fifo.to_str().unwrap(),
        ]);
        cmd.args(extra);
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
        writeln!(writer, "{}", path.display()).expect("write request");
    }

    fn wait_for(&self, expected_path: &Path) -> Fields {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            if let Ok(fields) = read_status(&self.status_file) {
                if fields.path.as_os_str() == expected_path.as_os_str() {
                    return fields;
                }
            }
            if Instant::now() >= deadline {
                panic!(
                    "daemon did not respond for {} within {:?}",
                    expected_path.display(),
                    RESPONSE_TIMEOUT
                );
            }
            std::thread::sleep(Duration::from_millis(10));
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
}

fn read_status(p: &Path) -> std::io::Result<Fields> {
    let mut buf = Vec::new();
    File::open(p)?.read_to_end(&mut buf)?;
    let parts: Vec<&[u8]> = buf.split(|&b| b == 0).collect();
    // Five fields followed by an empty trailing element from the final NUL.
    if parts.len() < 5 {
        return Err(std::io::Error::other("fewer than 5 fields"));
    }
    let s = |b: &[u8]| std::str::from_utf8(b).unwrap_or("").to_owned();
    Ok(Fields {
        path: PathBuf::from(s(parts[0])),
        branch: s(parts[1]),
        ahead: s(parts[2]),
        behind: s(parts[3]),
        dirty: s(parts[4]),
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
    assert_eq!(f.dirty, "1");
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
fn deadline_zero_forces_unknown() {
    let h = Harness::with_args(&["--dirty-deadline-ms", "0"]);
    let repo = make_dirty_repo();
    h.request(repo.path());
    let f = h.wait_for(repo.path());
    assert_eq!(f.branch, "main");
    assert_eq!(f.dirty, "?");
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
    assert_eq!(f.dirty, "1");

    h.request(none.path());
    let f = h.wait_for(none.path());
    assert_eq!(f.branch, "");
}
