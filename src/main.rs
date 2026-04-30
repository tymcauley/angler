use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

fn main() {
    let mut fish_pid: Option<i32> = None;
    let mut status_file: Option<PathBuf> = None;
    let mut request_fifo: Option<PathBuf> = None;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--fish-pid" => {
                fish_pid = args
                    .get(i + 1)
                    .and_then(|s| s.parse().ok());
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

    // Open the FIFO with O_RDWR so reads don't EOF every time fish closes its
    // write end. The daemon itself is always a writer, so the read side stays
    // open across many short fish-side writes. Cleanup is handled by the
    // watchdog thread (parent-death detection via getppid()).
    let fifo = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&request_fifo)
        .expect("failed to open request fifo");
    let reader = BufReader::new(fifo);

    for line in reader.lines() {
        let Ok(line) = line else { break };
        let path = PathBuf::from(line.trim());
        if path.as_os_str().is_empty() {
            continue;
        }
        let status = compute_status(&path);
        if let Err(e) = write_status_file(&status_file, &path, status.as_ref()) {
            eprintln!("fish-prompt-daemon: write_status_file: {e}");
            continue;
        }
        // SAFETY: kill(2) with a valid signal number is safe; pid is i32.
        unsafe { libc::kill(fish_pid, libc::SIGUSR1) };
    }
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

struct Status {
    branch: String,
    ahead: u32,
    behind: u32,
    dirty: bool,
}

fn compute_status(path: &Path) -> Option<Status> {
    let repo = gix::discover(path).ok()?;

    let branch = match repo.head_name().ok().flatten() {
        Some(name) => name.shorten().to_string(),
        None => match repo.head_id() {
            Ok(id) => id.to_hex_with_len(7).to_string(),
            Err(_) => "(detached)".to_string(),
        },
    };

    let dirty = repo.is_dirty().unwrap_or(false);

    Some(Status {
        branch,
        ahead: 0,
        behind: 0,
        dirty,
    })
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
                if s.dirty { 1 } else { 0 }
            )?;
        } else {
            f.write_all(b"\0\0\0\0")?;
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}
