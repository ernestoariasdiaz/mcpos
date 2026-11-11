//! mcp-explore: token-free exploration of desktop apps.
//!
//! An explorer process walks an app's accessibility tree choosing actions by novelty (UCB),
//! fuzzes text fields, and watches an oracle (crash with coredump backtrace, hang, non-zero
//! exit, journal errors). Findings are de-duplicated by stack signature and saved with the
//! steps to reproduce them. The agent only reads summaries: tokens per bug, not per click.
//!
//!   mcp-explore            MCP server (stdio)
//!   mcp-explore run DIR    explorer process (spawned by `start`)

mod explorer;

use mcp_core::json::{self, obj, Value};
use mcp_core::util::{schema, truncate, unix_now};
use mcp_core::{serve_stdio, ToolResult, Tools};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn base_dir() -> PathBuf {
    let state = std::env::var("XDG_STATE_HOME")
        .unwrap_or_else(|_| format!("{}/.local/state", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    PathBuf::from(state).join("mcpos-explore")
}

fn runs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(base_dir())
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join("config.json").exists()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn run_dir(a: &Value) -> Result<PathBuf, String> {
    match a.str_field("run") {
        Some(r) => {
            let p = base_dir().join(r);
            if p.join("config.json").exists() { Ok(p) } else { Err(format!("no run '{r}' (see explore__runs)")) }
        }
        None => runs().pop().ok_or_else(|| "no runs yet: use explore__start".into()),
    }
}

fn read_json(p: &Path) -> Option<Value> {
    std::fs::read_to_string(p).ok().and_then(|t| json::parse(&t).ok())
}

fn pid_alive(pid: i64) -> bool {
    pid > 0 && Path::new(&format!("/proc/{pid}")).exists()
}

fn status_text(dir: &Path) -> String {
    let name = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(s) = read_json(&dir.join("status.json")) else { return format!("{name}: starting…") };
    let running = s.bool_field("running").unwrap_or(false) && pid_alive(s.i64_field("explorer_pid").unwrap_or(0));
    let mins = (s.get("updated").and_then(|v| v.as_f64()).unwrap_or(0.0) - s.get("started").and_then(|v| v.as_f64()).unwrap_or(0.0)) / 60.0;
    format!(
        "{name}: {} — app '{}' — {:.1} min, {} steps, {} episodes, {} unique UI states, {} findings\n  last: {}",
        if running { "RUNNING" } else { "finished" },
        s.str_field("app").unwrap_or("?"),
        mins,
        s.i64_field("steps").unwrap_or(0),
        s.i64_field("episodes").unwrap_or(0),
        s.i64_field("states").unwrap_or(0),
        s.i64_field("findings").unwrap_or(0),
        s.str_field("last_event").unwrap_or(""),
    )
}

fn load_findings(dir: &Path) -> Vec<Value> {
    let mut v: Vec<Value> = std::fs::read_dir(dir.join("findings"))
        .map(|rd| rd.flatten().filter_map(|e| read_json(&e.path())).collect())
        .unwrap_or_default();
    let rank = |k: &str| match k { "crash" => 0, "hang" => 1, "exit" => 2, _ => 3 };
    v.sort_by(|a, b| rank(a.str_field("kind").unwrap_or("")).cmp(&rank(b.str_field("kind").unwrap_or("")))
        .then(b.i64_field("count").unwrap_or(0).cmp(&a.i64_field("count").unwrap_or(0))));
    v
}

fn start(a: &Value) -> ToolResult {
    let Some(app) = a.str_field("app") else { return ToolResult::error("missing 'app' (command line, e.g. 'kate -n' or 'dolphin --new-window')") };
    // fail fast if the program isn't installed (the explorer would otherwise die in the background)
    let prog = app.split_whitespace().next().unwrap_or("");
    let found = mcp_core::proc::run("sh", &["-c", "command -v -- \"$1\"", "sh", prog], None, Duration::from_secs(5))
        .map(|o| o.ok()).unwrap_or(false);
    if !found {
        return ToolResult::error(format!(
            "'{prog}' is not installed. Install it first, e.g. shell__exec(command=\"pacman -S --noconfirm {prog}\", as_root=true), \
             or pick an installed app: konsole, dolphin --new-window, kate -n, kcalc, kwrite, okular, gwenview, ark"
        ));
    }
    let minutes = a.get("minutes").and_then(|v| v.as_f64()).unwrap_or(10.0).clamp(0.2, 24.0 * 60.0);
    let slug: String = app.split_whitespace().next().unwrap_or("app").chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let id = format!("{}-{}", unix_now() as u64, slug);
    let dir = base_dir().join(&id);
    if let Err(e) = std::fs::create_dir_all(&dir) { return ToolResult::error(format!("mkdir: {e}")); }
    let mut cfg = obj([
        ("app", app.into()),
        ("minutes", minutes.into()),
        ("max_steps_per_episode", a.i64_field("max_steps_per_episode").unwrap_or(80).into()),
        ("step_delay_ms", a.i64_field("step_delay_ms").unwrap_or(400).into()),
        ("seed", a.i64_field("seed").unwrap_or((unix_now() as i64) & 0xffff_ffff).into()),
        ("sandbox_home", a.bool_field("sandbox_home").unwrap_or(true).into()),
        ("go_explore", a.bool_field("go_explore").unwrap_or(true).into()),
    ]);
    if let Some(d) = a.get("deny") { cfg.set("deny", d.clone()); }
    if let Err(e) = std::fs::write(dir.join("config.json"), cfg.to_json()) { return ToolResult::error(format!("config: {e}")); }

    let exe = match std::env::current_exe() { Ok(p) => p, Err(e) => return ToolResult::error(format!("current_exe: {e}")) };
    let log = match std::fs::File::create(dir.join("explorer.log")) { Ok(f) => f, Err(e) => return ToolResult::error(format!("log: {e}")) };
    let log2 = log.try_clone().unwrap();
    // own process group: survives this MCP server being restarted by mcpd
    match Command::new(exe).arg("run").arg(&dir)
        .stdin(Stdio::null()).stdout(log).stderr(log2).process_group(0).spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || { let _ = child.wait(); });
            ToolResult::text(format!(
                "exploration started: run={id}\napp: {app} · {minutes} min · sandboxed HOME: {}\n\
                 No tokens are spent while it runs. Check later with explore__status / explore__findings.",
                a.bool_field("sandbox_home").unwrap_or(true)
            ))
        }
        Err(e) => ToolResult::error(format!("spawn explorer: {e}")),
    }
}

fn stop(a: &Value) -> ToolResult {
    let dir = match run_dir(a) { Ok(d) => d, Err(e) => return ToolResult::error(e) };
    let _ = std::fs::write(dir.join("STOP"), "");
    for _ in 0..40 {
        let running = read_json(&dir.join("status.json"))
            .map(|s| s.bool_field("running").unwrap_or(false) && pid_alive(s.i64_field("explorer_pid").unwrap_or(0)))
            .unwrap_or(false);
        if !running { return ToolResult::text(format!("stopped\n{}", status_text(&dir))); }
        std::thread::sleep(Duration::from_millis(500));
    }
    ToolResult::text(format!("stop requested (explorer finishing its current step)\n{}", status_text(&dir)))
}

fn findings(a: &Value) -> ToolResult {
    let dirs: Vec<PathBuf> = if a.bool_field("all_runs").unwrap_or(false) { runs() } else {
        match run_dir(a) { Ok(d) => vec![d], Err(e) => return ToolResult::error(e) }
    };
    let mut out = String::new();
    for d in dirs {
        out.push_str(&status_text(&d));
        out.push('\n');
        let f = load_findings(&d);
        if f.is_empty() { out.push_str("  (no findings)\n"); }
        for v in f {
            out.push_str(&format!(
                "  [{}] {:<9} x{:<3} {}\n             {} · repro: {} steps\n",
                v.str_field("id").unwrap_or("?"),
                v.str_field("kind").unwrap_or("?"),
                v.i64_field("count").unwrap_or(1),
                truncate(v.str_field("signature").unwrap_or(""), 140),
                truncate(v.str_field("detail").unwrap_or(""), 140),
                v.get("repro").and_then(|r| r.as_array()).map(|r| r.len()).unwrap_or(0),
            ));
        }
    }
    out.push_str("\nUse explore__finding(id) for backtrace and steps, explore__replay(id) to confirm it reproduces.");
    ToolResult::text(out)
}

fn find_finding(a: &Value) -> Result<(PathBuf, Value), String> {
    let id = a.str_field("id").ok_or("missing 'id'")?;
    let dirs = match a.str_field("run") { Some(_) => vec![run_dir(a)?], None => runs() };
    for d in dirs.iter().rev() {
        if let Some(v) = read_json(&d.join("findings").join(format!("{id}.json"))) { return Ok((d.clone(), v)); }
    }
    Err(format!("no finding '{id}'"))
}

fn finding(a: &Value) -> ToolResult {
    match find_finding(a) {
        Err(e) => ToolResult::error(e),
        Ok((_, v)) => {
            let steps: Vec<String> = v.get("repro").and_then(|r| r.as_array()).into_iter().flatten().enumerate()
                .map(|(i, s)| format!("{:>3}. {} {} {:?}{}", i + 1,
                    s.str_field("action").unwrap_or(""), s.str_field("role").unwrap_or(""), s.str_field("name").unwrap_or(""),
                    s.str_field("text").map(|t| format!(" text={:?}", truncate(t, 60))).unwrap_or_default()))
                .collect();
            ToolResult::text(format!(
                "[{}] {} in '{}' — seen {}x\nsignature: {}\ndetail: {}\n\n--- steps to reproduce (episode {}) ---\n{}\n\n--- evidence ---\n{}",
                v.str_field("id").unwrap_or(""), v.str_field("kind").unwrap_or(""), v.str_field("app").unwrap_or(""),
                v.i64_field("count").unwrap_or(1), v.str_field("signature").unwrap_or(""), v.str_field("detail").unwrap_or(""),
                v.i64_field("episode").unwrap_or(0), steps.join("\n"), v.str_field("evidence").unwrap_or(""),
            ))
        }
    }
}

fn coverage(a: &Value) -> ToolResult {
    let dir = match run_dir(a) { Ok(d) => d, Err(e) => return ToolResult::error(e) };
    let Some(c) = read_json(&dir.join("coverage.json")) else { return ToolResult::error("no coverage yet (written after each episode)") };
    let mut out = format!("{}\n\nUI states reached (shortest known path from a fresh start):\n", status_text(&dir));
    for st in c.get("states").and_then(|x| x.as_array()).into_iter().flatten() {
        let path: Vec<&str> = st.get("path").and_then(|p| p.as_array()).into_iter().flatten().filter_map(|x| x.as_str()).collect();
        out.push_str(&format!("  [{}] depth {} · used as start {}× · {}\n",
            st.str_field("state").unwrap_or("?"), st.i64_field("depth").unwrap_or(0), st.i64_field("chosen").unwrap_or(0),
            if path.is_empty() { "(start screen)".to_string() } else { truncate(&path.join(" → "), 300) }));
    }
    out.push_str("\nHow often each kind of control led somewhere new:\n");
    for r in c.get("controls").and_then(|x| x.as_array()).into_iter().flatten() {
        out.push_str(&format!("  {:<28} {:>5} tries  {:>5.1}% novel\n", r.str_field("control").unwrap_or("?"),
            r.get("tries").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64, 100.0 * r.get("novelty_rate").and_then(|v| v.as_f64()).unwrap_or(0.0)));
    }
    let list = |k: &str| c.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n  ")).unwrap_or_default();
    out.push_str(&format!("\nMost tried:\n  {}\n\nSeen but never tried ({} of {} distinct actions tried):\n  {}\n",
        list("most_tried"), c.i64_field("distinct_actions_tried").unwrap_or(0), c.i64_field("distinct_actions_seen").unwrap_or(0), list("never_tried")));
    out.push_str("\nHints: few states + high tries on menus with 0% novelty = menus don't open via AT-SPI for this toolkit; \
                  many actions never tried = raise minutes or max_steps_per_episode.");
    ToolResult::text(out)
}

fn replay(a: &Value) -> ToolResult {
    let (dir, v) = match find_finding(a) { Ok(x) => x, Err(e) => return ToolResult::error(e) };
    let times = a.i64_field("times").unwrap_or(1).clamp(1, 5);
    let mut out = Vec::new();
    let mut hits = 0;
    for i in 0..times {
        match explorer::replay(&dir, &v, Duration::from_millis(500)) {
            Ok(t) => { if t.starts_with("CRASH") || t.starts_with("HANG") || t.starts_with("LOG-ERROR") { hits += 1; } out.push(format!("--- attempt {} ---\n{t}", i + 1)); }
            Err(e) => out.push(format!("--- attempt {} --- error: {e}", i + 1)),
        }
    }
    ToolResult::text(format!("reproduced {hits}/{times}\n{}", out.join("\n")))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("run") {
        let dir = PathBuf::from(args.get(2).expect("usage: mcp-explore run DIR"));
        if let Err(e) = explorer::explore(&dir) {
            eprintln!("explorer: {e}");
            std::process::exit(1);
        }
        return;
    }
    let mut tools = Tools::new()
        .tool("start",
            "Start a background, token-free exploration of a desktop app. The explorer clicks and types through the app's \
             accessibility tree, preferring actions that reach new UI states, fuzzes text fields, and records crashes \
             (with backtrace), hangs, abnormal exits and error logs. Destructive actions (delete, trash, quit, log out…) \
             are never triggered and the app runs with a throw-away HOME. For single-instance apps pass a new-instance \
             flag, e.g. 'kate -n'.",
            schema(&[
                ("app", "string", "Command line to launch, e.g. 'kate -n', 'dolphin --new-window', 'kcalc'"),
                ("minutes", "number", "How long to explore (default 10)"),
                ("max_steps_per_episode", "integer", "Restart the app after this many actions (default 80)"),
                ("step_delay_ms", "integer", "Wait after each action (default 400)"),
                ("seed", "integer", "RNG seed, for reproducible runs"),
                ("sandbox_home", "boolean", "Run the app with a throw-away HOME (default true)"),
                ("deny", "array", "Extra substrings of element names never to click"),
                ("go_explore", "boolean", "Return to rarely-visited known states before exploring (default true)"),
            ], &["app"]), start)
        .tool("status", "Progress of an exploration run (latest if 'run' omitted).",
            schema(&[("run", "string", "Run id")], &[]),
            |a| match run_dir(a) { Ok(d) => ToolResult::text(status_text(&d)), Err(e) => ToolResult::error(e) })
        .tool("runs", "List exploration runs.", schema(&[], &[]), |_| {
            let r = runs();
            if r.is_empty() { return ToolResult::text("no runs yet"); }
            ToolResult::text(r.iter().map(|d| status_text(d)).collect::<Vec<_>>().join("\n"))
        })
        .tool("stop", "Stop a running exploration.", schema(&[("run", "string", "Run id")], &[]), stop)
        .tool("findings", "Unique findings (crash, hang, exit, log-error) with counts, most severe first.",
            schema(&[("run", "string", "Run id"), ("all_runs", "boolean", "Across all runs")], &[]), findings)
        .tool("finding", "One finding in full: signature, backtrace / evidence and the steps that led to it.",
            schema(&[("id", "string", "Finding id"), ("run", "string", "Run id")], &["id"]), finding)
        .tool("coverage", "What the explorer reached: UI states with the path to each, how often each kind of control \
             (button, menu, tab…) led somewhere new, and actions seen but never tried. Use it when a run finds little.",
            schema(&[("run", "string", "Run id")], &[]), coverage)
        .tool("replay", "Replay a finding's steps on a fresh instance to check it reproduces (crash/hang).",
            schema(&[("id", "string", "Finding id"), ("run", "string", "Run id"), ("times", "integer", "Attempts (1-5)")], &["id"]), replay);

    serve_stdio(
        "mcp-explore",
        env!("CARGO_PKG_VERSION"),
        Some("Token-free bug hunting: explore__start an app, come back later, read explore__findings, \
              then spend effort only on unique crashes/hangs (explore__finding, explore__replay, journal__coredump_backtrace)."),
        &mut tools,
    );
}
