//! Running external commands with a timeout, capturing output.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub elapsed: Duration,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
    /// Human/LLM-friendly rendering.
    pub fn render(&self, max_bytes: usize) -> String {
        let mut s = String::new();
        if self.timed_out {
            s.push_str("[timed out — process group killed]\n");
        }
        s.push_str(&format!(
            "exit: {}  ({} ms)\n",
            self.code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            self.elapsed.as_millis()
        ));
        if !self.stdout.is_empty() {
            s.push_str("--- stdout ---\n");
            s.push_str(&crate::util::truncate(&self.stdout, max_bytes));
            if !s.ends_with('\n') { s.push('\n'); }
        }
        if !self.stderr.is_empty() {
            s.push_str("--- stderr ---\n");
            s.push_str(&crate::util::truncate(&self.stderr, max_bytes / 2));
        }
        s
    }
}

/// Run `program args...`, optionally feeding stdin, killing the whole process group on timeout.
pub fn run(program: &str, args: &[&str], stdin: Option<&[u8]>, timeout: Duration) -> Result<Output, String> {
    run_in(program, args, stdin, timeout, None)
}

pub fn run_in(
    program: &str,
    args: &[&str],
    stdin: Option<&[u8]>,
    timeout: Duration,
    cwd: Option<&str>,
) -> Result<Output, String> {
    let start = Instant::now();
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn {program}: {e}"))?;

    if let Some(data) = stdin {
        let mut si = child.stdin.take().unwrap();
        let data = data.to_vec();
        thread::spawn(move || {
            let _ = si.write_all(&data);
        });
    }
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let to = thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.read_to_end(&mut v);
        v
    });
    let te = thread::spawn(move || {
        let mut v = Vec::new();
        let _ = se.read_to_end(&mut v);
        v
    });

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    let pgid = format!("-{}", child.id());
                    let _ = Command::new("kill").args(["-KILL", "--", &pgid]).status();
                    let _ = child.kill();
                    break child.wait().ok();
                }
                thread::sleep(Duration::from_millis(15));
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    };
    let stdout = String::from_utf8_lossy(&to.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&te.join().unwrap_or_default()).into_owned();
    Ok(Output {
        code: status.and_then(|s| s.code()),
        stdout,
        stderr,
        timed_out,
        elapsed: start.elapsed(),
    })
}

/// `sh -c script` with timeout.
pub fn sh(script: &str, timeout: Duration) -> Result<Output, String> {
    run("sh", &["-c", script], None, timeout)
}

/// True if passwordless sudo works (cached).
pub fn have_sudo() -> bool {
    static S: OnceLock<bool> = OnceLock::new();
    *S.get_or_init(|| {
        if is_root() {
            return true;
        }
        run("sudo", &["-n", "true"], None, Duration::from_secs(5))
            .map(|o| o.ok())
            .unwrap_or(false)
    })
}

pub fn is_root() -> bool {
    static R: OnceLock<bool> = OnceLock::new();
    *R.get_or_init(|| {
        run("id", &["-u"], None, Duration::from_secs(5))
            .map(|o| o.stdout.trim() == "0")
            .unwrap_or(false)
    })
}

/// Build argv, prefixing `sudo -n` when asked and we aren't root already.
pub fn maybe_root(as_root: bool, argv: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if as_root && !is_root() {
        v.extend(["sudo", "-n", "--"].iter().map(|s| s.to_string()));
    }
    v.extend(argv.iter().map(|s| s.to_string()));
    v
}

/// Run an owned argv.
pub fn run_argv(argv: &[String], stdin: Option<&[u8]>, timeout: Duration) -> Result<Output, String> {
    let (p, rest) = argv.split_first().ok_or("empty argv")?;
    let rest: Vec<&str> = rest.iter().map(|s| s.as_str()).collect();
    run(p, &rest, stdin, timeout)
}
