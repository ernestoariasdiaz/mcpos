//! mcp-shell: unrestricted command execution (as user or root) for mcpOS.

use mcp_core::proc::{self, is_root, maybe_root, run_argv};
use mcp_core::util::{schema, truncate};
use mcp_core::{serve_stdio, ToolResult, Tools, Value};
use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

const MAX_OUT: usize = 60_000;

fn exec(a: &Value) -> ToolResult {
    let Some(cmd) = a.str_field("command") else { return ToolResult::error("missing 'command'") };
    let timeout = Duration::from_secs(a.i64_field("timeout_secs").unwrap_or(120).clamp(1, 3600) as u64);
    let stdin = a.str_field("stdin").map(|s| s.as_bytes().to_vec());

    // cd inside the shell so it works under sudo too
    let script = match a.str_field("cwd") {
        Some(d) => format!("cd {} && {}", sh_quote(d), cmd),
        None => cmd.to_string(),
    };
    let argv: Vec<String> = match a.str_field("user") {
        Some(u) => ["sudo", "-n", "-u", u, "--", "sh", "-c", &script].iter().map(|s| s.to_string()).collect(),
        None => maybe_root(a.bool_field("as_root").unwrap_or(false), &["sh", "-c", &script]),
    };
    match run_argv(&argv, stdin.as_deref(), timeout) {
        Ok(o) => {
            let r = ToolResult::text(o.render(MAX_OUT));
            if o.timed_out { ToolResult { is_error: true, ..r } } else { r }
        }
        Err(e) => ToolResult::error(e),
    }
}

fn spawn(a: &Value) -> ToolResult {
    let Some(cmd) = a.str_field("command") else { return ToolResult::error("missing 'command'") };
    let argv = maybe_root(a.bool_field("as_root").unwrap_or(false), &["sh", "-c", cmd]);
    let log = format!(
        "/tmp/mcp-spawn-{}.log",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
    );
    let f = match OpenOptions::new().create(true).append(true).open(&log) {
        Ok(f) => f,
        Err(e) => return ToolResult::error(format!("open log: {e}")),
    };
    let f2 = f.try_clone().unwrap();
    let mut c = Command::new(&argv[0]);
    c.args(&argv[1..]).stdin(Stdio::null()).stdout(f).stderr(f2).process_group(0);
    match c.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            std::thread::spawn(move || { let _ = child.wait(); }); // reap
            ToolResult::text(format!("started pid {pid} (pgid {pid})\nlog: {log}\nstop with: kill -- -{pid}"))
        }
        Err(e) => ToolResult::error(format!("spawn: {e}")),
    }
}

fn read_log(a: &Value) -> ToolResult {
    let Some(p) = a.str_field("path") else { return ToolResult::error("missing 'path'") };
    let tail = a.i64_field("tail_bytes").unwrap_or(20_000).max(1) as usize;
    match std::fs::read(p) {
        Ok(b) => {
            let start = b.len().saturating_sub(tail);
            ToolResult::text(String::from_utf8_lossy(&b[start..]).into_owned())
        }
        Err(e) => ToolResult::error(format!("{p}: {e}")),
    }
}

fn system_info(_: &Value) -> ToolResult {
    let script = r#"echo "== uname"; uname -a
echo "== os"; . /etc/os-release 2>/dev/null && echo "$PRETTY_NAME"
echo "== whoami"; id
echo "== uptime"; uptime
echo "== mem"; free -h
echo "== disk"; df -h / /home 2>/dev/null
echo "== cpu"; nproc; grep -m1 'model name' /proc/cpuinfo
echo "== session"; echo "XDG_SESSION_TYPE=$XDG_SESSION_TYPE WAYLAND_DISPLAY=$WAYLAND_DISPLAY SWAYSOCK=$SWAYSOCK""#;
    match proc::sh(script, Duration::from_secs(15)) {
        Ok(o) => ToolResult::text(format!(
            "{}\n== mcp-shell\nroot={} passwordless_sudo={}",
            truncate(&o.stdout, MAX_OUT),
            is_root(),
            proc::have_sudo()
        )),
        Err(e) => ToolResult::error(e),
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn main() {
    let mut tools = Tools::new()
        .tool(
            "exec",
            "Run a shell command (sh -c) and return exit code, stdout and stderr. \
             Set as_root=true to run via passwordless sudo, or user=<name> to run as another user. \
             Use for anything: packages, files, services, sysctl, kernel modules.",
            schema(
                &[
                    ("command", "string", "Shell command line"),
                    ("cwd", "string", "Working directory"),
                    ("as_root", "boolean", "Run as root via sudo -n"),
                    ("user", "string", "Run as this user via sudo -n -u"),
                    ("timeout_secs", "integer", "Kill after N seconds (default 120, max 3600)"),
                    ("stdin", "string", "Data to feed on stdin"),
                ],
                &["command"],
            ),
            exec,
        )
        .tool(
            "spawn",
            "Start a long-running/background process (e.g. a GUI app or a server) without waiting. \
             Output goes to a log file; returns pid and log path.",
            schema(
                &[("command", "string", "Shell command line"), ("as_root", "boolean", "Run as root via sudo -n")],
                &["command"],
            ),
            spawn,
        )
        .tool(
            "read_log",
            "Read the tail of a file, e.g. the log of a spawned process.",
            schema(&[("path", "string", "File path"), ("tail_bytes", "integer", "Bytes from the end (default 20000)")], &["path"]),
            read_log,
        )
        .tool("system_info", "Kernel, OS, identity, memory, disk, CPU and session variables.", schema(&[], &[]), system_info);

    serve_stdio(
        "mcp-shell",
        env!("CARGO_PKG_VERSION"),
        Some("Unrestricted shell on an mcpOS test VM. Root is allowed via as_root."),
        &mut tools,
    );
}
