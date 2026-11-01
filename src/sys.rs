//! Process, filesystem and terminal helpers.
//!
//! Kept dependency-free: `which`, folding and a few small utilities are
//! implemented here so the binary builds offline.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Shorthand for building a single argument.
pub fn os(s: &str) -> OsString {
    OsString::from(s)
}

/// Shorthand for building argument vectors out of string literals.
pub fn argv(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// Look `name` up in `PATH`, returning the first executable match.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn program_name<S: AsRef<OsStr>>(program: S) -> String {
    program.as_ref().to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// child tracking + signal forwarding
// ---------------------------------------------------------------------------

/// Maximum number of simultaneously tracked children.
const MAX_TRACKED_CHILDREN: usize = 32;

#[cfg(unix)]
mod platform {
    use std::process::Command;
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::MAX_TRACKED_CHILDREN;

    const SIGHUP: i32 = 1;
    const SIGINT: i32 = 2;
    const SIGQUIT: i32 = 3;
    const SIGTERM: i32 = 15;
    const SIG_DFL: usize = 0;

    static CHILDREN: [AtomicI32; MAX_TRACKED_CHILDREN] =
        [const { AtomicI32::new(0) }; MAX_TRACKED_CHILDREN];

    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
        fn kill(pid: i32, sig: i32) -> i32;
        fn getpid() -> i32;
    }

    /// Forward the signal to every tracked child, then die from it ourselves.
    extern "C" fn forward(sig: i32) {
        for slot in CHILDREN.iter() {
            let pid = slot.load(Ordering::Relaxed);
            if pid > 0 {
                // SAFETY: `kill` is async-signal-safe.
                unsafe {
                    kill(pid, sig);
                }
            }
        }
        // SAFETY: restore the default action and re-raise so the process
        // terminates exactly as it would have without the handler.
        unsafe {
            signal(sig, SIG_DFL);
            kill(getpid(), sig);
        }
    }

    pub fn install() {
        unsafe {
            let handler: extern "C" fn(i32) = forward;
            for sig in [SIGHUP, SIGINT, SIGQUIT, SIGTERM] {
                signal(sig, handler as usize);
            }
        }
    }

    pub fn register(pid: u32) -> usize {
        let pid = pid as i32;
        for (index, slot) in CHILDREN.iter().enumerate() {
            if slot
                .compare_exchange(0, pid, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return index;
            }
        }
        usize::MAX
    }

    pub fn unregister(slot: usize) {
        if let Some(slot) = CHILDREN.get(slot) {
            slot.store(0, Ordering::SeqCst);
        }
    }

    /// Make children die together with this process, even if it is SIGKILLed.
    #[cfg(target_os = "linux")]
    pub fn arm_parent_death_signal(command: &mut Command) {
        use std::os::raw::c_ulong;
        use std::os::unix::process::CommandExt;

        const PR_SET_PDEATHSIG: i32 = 1;

        extern "C" {
            fn prctl(option: i32, ...) -> i32;
            fn getppid() -> i32;
            fn _exit(status: i32) -> !;
        }

        // SAFETY: the closure only calls async-signal-safe libc functions and
        // performs no allocation.
        unsafe {
            command.pre_exec(|| {
                let _ = prctl(PR_SET_PDEATHSIG, SIGTERM as c_ulong);
                if getppid() == 1 {
                    _exit(1);
                }
                Ok(())
            });
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn arm_parent_death_signal(_command: &mut Command) {}
}

#[cfg(not(unix))]
mod platform {
    use std::process::Command;

    pub fn install() {}
    pub fn register(_pid: u32) -> usize {
        0
    }
    pub fn unregister(_slot: usize) {}
    pub fn arm_parent_death_signal(_command: &mut Command) {}
}

/// Forward SIGINT/SIGTERM/SIGHUP/SIGQUIT to spawned children (Unix only).
pub fn install_signal_forwarding() {
    platform::install();
}

/// A spawned child registered in the global table used by the signal handler.
pub struct TrackedChild {
    child: Child,
    slot: usize,
}

impl TrackedChild {
    pub fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub fn wait(mut self) -> std::io::Result<ExitStatus> {
        let result = self.child.wait();
        platform::unregister(self.slot);
        result
    }

    pub fn wait_with_output(self) -> std::io::Result<Output> {
        let Self { child, slot } = self;
        let result = child.wait_with_output();
        platform::unregister(slot);
        result
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }
}

fn spawn_tracked(mut command: Command) -> std::io::Result<TrackedChild> {
    platform::arm_parent_death_signal(&mut command);
    let child = command.spawn()?;
    let slot = platform::register(child.id());
    Ok(TrackedChild { child, slot })
}

/// Run a command with no stdin, capturing stdout/stderr.
pub fn run<S: AsRef<OsStr>>(program: S, args: &[OsString]) -> Result<Output, String> {
    let name = program_name(&program);
    let mut command = Command::new(program.as_ref());
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = spawn_tracked(command).map_err(|e| format!("{name}: {e}"))?;
    child.wait_with_output().map_err(|e| format!("{name}: {e}"))
}

/// Run a command feeding `input` on stdin.
///
/// The write happens on a helper thread so the child cannot deadlock on a full
/// stdout pipe while we are still writing its stdin.
pub fn run_with_stdin<S: AsRef<OsStr>>(
    program: S,
    args: &[OsString],
    input: &[u8],
) -> Result<Output, String> {
    let name = program_name(&program);
    let mut command = Command::new(program.as_ref());
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn_tracked(command).map_err(|e| format!("{name}: {e}"))?;

    let mut stdin = child.take_stdin().expect("piped stdin");
    let data = input.to_vec();
    let writer = std::thread::spawn(move || {
        // A broken pipe just means the child stopped reading; that is its
        // business and the exit status will tell us what happened.
        let _ = stdin.write_all(&data);
    });

    let output = child
        .wait_with_output()
        .map_err(|e| format!("{name}: {e}"))?;
    let _ = writer.join();
    Ok(output)
}

pub fn stderr_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim_end().to_string()
}

fn status_text(out: &Output) -> String {
    match out.status.code() {
        Some(code) => format!("command failed with status {code}"),
        None => "command was terminated by a signal".to_string(),
    }
}

/// Error text for a failed command: stderr if there is any, else the status.
pub fn err_text(out: &Output) -> String {
    let msg = stderr_text(out);
    if msg.is_empty() {
        status_text(out)
    } else {
        msg
    }
}

/// Like [`err_text`], but also treats "successful with warnings" as an error
/// message so the warnings can be surfaced to the user.
pub fn note_text(out: &Output) -> Option<String> {
    let msg = stderr_text(out);
    if !msg.is_empty() {
        Some(msg)
    } else if !out.status.success() {
        Some(status_text(out))
    } else {
        None
    }
}

/// Run and require a zero exit status; stderr on success is ignored.
pub fn status_of<S: AsRef<OsStr>>(program: S, args: &[OsString]) -> Result<Output, String> {
    let out = run(program, args)?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(err_text(&out))
    }
}

/// Run, require success and surface warnings printed on stderr.
pub fn status_checked<S: AsRef<OsStr>>(program: S, args: &[OsString]) -> Result<(), String> {
    let out = run(program, args)?;
    match note_text(&out) {
        Some(msg) => Err(msg),
        None => Ok(()),
    }
}

/// Run and return stdout, failing on a non-zero exit status.
pub fn stdout_of<S: AsRef<OsStr>>(program: S, args: &[OsString]) -> Result<Vec<u8>, String> {
    let out = run(program, args)?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(err_text(&out))
    }
}

/// Run and return stdout, feeding `input` on stdin.
pub fn stdout_with_stdin<S: AsRef<OsStr>>(
    program: S,
    args: &[OsString],
    input: &[u8],
) -> Result<Vec<u8>, String> {
    let out = run_with_stdin(program, args, input)?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(err_text(&out))
    }
}

pub fn write_file(dest: &Path, data: &[u8]) -> Result<(), String> {
    fs::write(dest, data).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Redirect a command's stdout straight into `dest` (never through memory).
///
/// The file is created even when the command fails, mirroring the shell
/// version's `cmd > "$CACHE"` behaviour; warnings/errors are returned as an
/// `Err` so the caller can print them.
pub fn write_stdout_to<S: AsRef<OsStr>>(
    program: S,
    args: &[OsString],
    dest: &Path,
) -> Result<(), String> {
    let name = program_name(&program);
    let file = fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut command = Command::new(program.as_ref());
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped());
    let child = spawn_tracked(command).map_err(|e| format!("{name}: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{name}: {e}"))?;
    match note_text(&out) {
        Some(msg) => Err(msg),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// streaming
// ---------------------------------------------------------------------------

/// One stage of a shell-style pipeline.
pub type Stage = (OsString, Vec<OsString>);

pub fn stage<S: AsRef<OsStr>>(program: S, args: Vec<OsString>) -> Stage {
    (program.as_ref().to_owned(), args)
}

/// Run `stages` connected like `a | b | c`, with all stderr inherited.
///
/// The last stage writes either to `out` (used for cache generation) or to our
/// stdout, so output reaches yazi while the pipeline is still running. Non-zero
/// exit statuses are not reported here: stderr is inherited, and the shell
/// version did not add extra messages either.
pub fn pipeline(stages: &[Stage], out: Option<&Path>) -> Result<(), String> {
    if stages.is_empty() {
        return Ok(());
    }

    let mut previous: Option<ChildStdout> = None;
    let mut children: Vec<(String, TrackedChild)> = Vec::with_capacity(stages.len());

    for (index, (program, args)) in stages.iter().enumerate() {
        let name = program.to_string_lossy().into_owned();
        let mut command = Command::new(program);
        command.args(args).stderr(Stdio::inherit());

        match previous.take() {
            Some(pipe) => {
                command.stdin(Stdio::from(pipe));
            }
            None => {
                command.stdin(Stdio::null());
            }
        }

        if index + 1 == stages.len() {
            match out {
                Some(path) => {
                    let file =
                        fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
                    command.stdout(Stdio::from(file));
                }
                None => {
                    command.stdout(Stdio::inherit());
                }
            }
        } else {
            command.stdout(Stdio::piped());
        }

        match spawn_tracked(command) {
            Ok(mut child) => {
                previous = child.take_stdout();
                children.push((name, child));
            }
            Err(e) => {
                for (_, mut child) in children {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err(format!("{name}: {e}"));
            }
        }
    }

    for (name, child) in children {
        if let Err(e) = child.wait() {
            return Err(format!("{name}: {e}"));
        }
    }
    Ok(())
}

/// Run a command with stdout/stderr inherited (no buffering at all).
pub fn stream<S: AsRef<OsStr>>(program: S, args: &[OsString]) -> Result<i32, String> {
    let name = program_name(&program);
    let mut command = Command::new(program.as_ref());
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let child = spawn_tracked(command).map_err(|e| format!("{name}: {e}"))?;
    let status = child.wait().map_err(|e| format!("{name}: {e}"))?;
    Ok(status.code().unwrap_or(0))
}

/// Feed `input` on stdin while the command writes directly to our stdout.
pub fn stream_with_stdin<S: AsRef<OsStr>>(
    program: S,
    args: &[OsString],
    input: &[u8],
) -> Result<i32, String> {
    let name = program_name(&program);
    let mut command = Command::new(program.as_ref());
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = spawn_tracked(command).map_err(|e| format!("{name}: {e}"))?;

    let mut stdin = child.take_stdin().expect("piped stdin");
    let data = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });

    let status = child.wait().map_err(|e| format!("{name}: {e}"))?;
    let _ = writer.join();
    Ok(status.code().unwrap_or(0))
}

/// Use `input` as the command's stdin, streaming its stdout to ours.
pub fn stream_from_file<S: AsRef<OsStr>>(
    program: S,
    args: &[OsString],
    input: &Path,
) -> Result<i32, String> {
    let name = program_name(&program);
    let file = fs::File::open(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut command = Command::new(program.as_ref());
    command
        .args(args)
        .stdin(Stdio::from(file))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let child = spawn_tracked(command).map_err(|e| format!("{name}: {e}"))?;
    let status = child.wait().map_err(|e| format!("{name}: {e}"))?;
    Ok(status.code().unwrap_or(0))
}

/// Copy a file to stdout in chunks (used for cached text previews).
pub fn stream_file(path: &Path) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    std::io::copy(&mut file, &mut lock).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = lock.flush();
    Ok(())
}

// ---------------------------------------------------------------------------
// terminal / text helpers
// ---------------------------------------------------------------------------

pub fn write_stdout(data: &[u8]) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(data);
    let _ = lock.flush();
}

/// Wrap every line at `width` characters, like `fold -w`.
pub fn fold(text: &str, width: usize) -> String {
    let width = width.max(1);
    let mut out = String::with_capacity(text.len() + text.len() / width + 1);
    for (line_no, line) in text.split('\n').enumerate() {
        if line_no > 0 {
            out.push('\n');
        }
        let mut col = 0usize;
        for ch in line.chars() {
            if col == width {
                out.push('\n');
                col = 0;
            }
            out.push(ch);
            col += 1;
        }
    }
    out
}

pub fn echo_err(msg: &str, width: Option<usize>) {
    let msg = msg.trim_end_matches(['\n', '\r']);
    let folded = match width {
        Some(width) => fold(msg, width),
        None => msg.to_string(),
    };
    let _ = write!(std::io::stderr(), "{folded}\n");
}

pub fn die(msg: &str, width: Option<usize>) -> ! {
    let folded = match width {
        Some(width) => fold(msg, width),
        None => msg.to_string(),
    };
    let _ = write!(std::io::stderr(), "\x1b[1;31m{folded}\x1b[m\n\r");
    let _ = std::io::stderr().flush();
    let _ = std::io::stdout().flush();
    std::process::exit(255);
}

/// Drop the first line, like `tail -n +2`.
pub fn drop_first_line(data: &[u8]) -> &[u8] {
    match data.iter().position(|b| *b == b'\n') {
        Some(i) => &data[i + 1..],
        None => &[],
    }
}

/// A unique, not-yet-existing path in the system temp directory.
pub fn temp_path(extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    for attempt in 0..1000u32 {
        let candidate =
            std::env::temp_dir().join(format!("preview-{pid}-{nanos}-{attempt}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    std::env::temp_dir().join(format!("preview-{pid}-{nanos}.{extension}"))
}
