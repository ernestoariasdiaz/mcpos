//! mcp-journal: logs, boots, failed units and coredumps — the bug-hunting view of the system.

use mcp_core::proc::{have_sudo, maybe_root, run_argv};
use mcp_core::util::schema;
use mcp_core::{serve_stdio, ToolResult, Tools, Value};
use std::time::Duration;

const MAX_OUT: usize = 80_000;

/// Run a systemd tool, as root when possible so we see every unit's logs.
fn sysrun(argv: &[&str], timeout_s: u64) -> ToolResult {
    let full = maybe_root(have_sudo(), argv);
    match run_argv(&full, None, Duration::from_secs(timeout_s)) {
        Ok(o) => {
            // journalctl returns 1 with "-- No entries --"; that's not an error for us
            let mut r = ToolResult::text(o.render(MAX_OUT));
            r.is_error = o.timed_out || (o.code != Some(0) && o.stdout.is_empty() && !o.stderr.contains("No entries"));
            r
        }
        Err(e) => ToolResult::error(e),
    }
}

fn query(a: &Value) -> ToolResult {
    let mut v: Vec<String> = vec!["journalctl".into(), "--no-pager".into(), "-o".into()];
    v.push(a.str_field("output").unwrap_or("short-iso-precise").into());
    let lines = a.i64_field("lines").unwrap_or(200).clamp(1, 20_000);
    v.push(format!("-n{lines}"));
    if let Some(u) = a.str_field("unit") { v.push(format!("--unit={u}")); }
    if let Some(u) = a.str_field("user_unit") { v.push(format!("--user-unit={u}")); }
    if let Some(p) = a.str_field("priority") { v.push(format!("--priority={p}")); }
    if let Some(s) = a.str_field("since") { v.push(format!("--since={s}")); }
    if let Some(s) = a.str_field("until") { v.push(format!("--until={s}")); }
    if let Some(g) = a.str_field("grep") { v.push(format!("--grep={g}")); v.push("--case-sensitive=false".into()); }
    if let Some(c) = a.str_field("comm") { v.push(format!("_COMM={c}")); }
    if let Some(p) = a.i64_field("pid") { v.push(format!("_PID={p}")); }
    if let Some(b) = a.i64_field("boot") { v.push(format!("--boot={b}")); }
    if a.bool_field("kernel").unwrap_or(false) { v.push("--dmesg".into()); }
    if a.bool_field("reverse").unwrap_or(false) { v.push("--reverse".into()); }
    let argv: Vec<&str> = v.iter().map(|s| s.as_str()).collect();
    sysrun(&argv, 60)
}

fn main() {
    let mut tools = Tools::new()
        .tool(
            "query",
            "Query the systemd journal. Filters combine (AND). Examples: priority='err' for errors only, \
             unit='sway' or user_unit='...' for one service, kernel=true for dmesg, comm='firefox' for one binary, \
             since='-10min' or '2026-11-09 13:00'.",
            schema(
                &[
                    ("unit", "string", "System unit, e.g. NetworkManager.service"),
                    ("user_unit", "string", "User unit"),
                    ("priority", "string", "emerg|alert|crit|err|warning|notice|info|debug or range 'err..warning'"),
                    ("since", "string", "Start time (journalctl syntax)"),
                    ("until", "string", "End time"),
                    ("grep", "string", "Regex on MESSAGE (case-insensitive)"),
                    ("comm", "string", "Executable name (_COMM)"),
                    ("pid", "integer", "Process id (_PID)"),
                    ("boot", "integer", "0 = current boot, -1 = previous ..."),
                    ("kernel", "boolean", "Kernel messages only"),
                    ("lines", "integer", "Max lines, newest (default 200)"),
                    ("reverse", "boolean", "Newest first"),
                    ("output", "string", "journalctl -o format (default short-iso-precise; 'json' for structured)"),
                ],
                &[],
            ),
            query,
        )
        .tool("list_boots", "List recorded boots (to compare a broken boot with a good one).", schema(&[], &[]),
            |_| sysrun(&["journalctl", "--list-boots", "--no-pager"], 30))
        .tool("failed_units", "System and user units in failed state.", schema(&[], &[]), |_| {
            let s = sysrun(&["systemctl", "--failed", "--no-pager", "--no-legend"], 20);
            let u = match run_argv(&["systemctl".into(), "--user".into(), "--failed".into(), "--no-pager".into(), "--no-legend".into()], None, Duration::from_secs(20)) {
                Ok(o) => o.render(MAX_OUT),
                Err(e) => e,
            };
            let sys = s.content.first().and_then(|c| c.str_field("text")).unwrap_or("").to_string();
            ToolResult::text(format!("== system\n{sys}\n== user\n{u}"))
        })
        .tool(
            "coredumps",
            "List crashes captured by systemd-coredump.",
            schema(&[("since", "string", "Only after this time, e.g. -1h")], &[]),
            |a| match a.str_field("since") {
                Some(s) => sysrun(&["coredumpctl", "list", "--no-pager", &format!("--since={s}")], 30),
                None => sysrun(&["coredumpctl", "list", "--no-pager"], 30),
            },
        )
        .tool(
            "coredump_info",
            "Details and stack trace of one crash. match = PID, executable name or path (latest match wins).",
            schema(&[("match", "string", "PID, COMM or exe path")], &["match"]),
            |a| match a.str_field("match") {
                Some(m) => sysrun(&["coredumpctl", "info", "--no-pager", m], 60),
                None => ToolResult::error("missing 'match'"),
            },
        )
        .tool(
            "coredump_backtrace",
            "Full gdb backtrace (all threads) of the latest crash matching 'match'. Needs gdb.",
            schema(&[("match", "string", "PID, COMM or exe path")], &["match"]),
            |a| match a.str_field("match") {
                Some(m) => sysrun(
                    &["coredumpctl", "debug", "--no-pager", m, "--debugger-arguments=-batch -ex 'thread apply all bt full'"],
                    180,
                ),
                None => ToolResult::error("missing 'match'"),
            },
        );

    serve_stdio(
        "mcp-journal",
        env!("CARGO_PKG_VERSION"),
        Some("Journal, boots, failed units and coredumps. Start bug hunts with query(priority='err', since='-15min')."),
        &mut tools,
    );
}
