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

fn wiki_dir() -> PathBuf {
    std::env::var("MCPOS_WIKI").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join("mcpos-wiki")
    })
}

/// The ```mcpos-explore {json}``` block of wiki/apps/<slug>.md, if any.
fn wiki_hints(slug: &str) -> Option<Value> {
    let text = std::fs::read_to_string(wiki_dir().join("apps").join(format!("{slug}.md"))).ok()?;
    let start = text.find("```mcpos-explore")?;
    let body = &text[start + "```mcpos-explore".len()..];
    let end = body.find("```")?;
    json::parse(body[..end].trim()).ok()
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
        "{name}: {} — app '{}' — {:.1} min, {} steps, {} episodes, {} unique UI states, actions tried {}/{}, {} findings\n  last: {}",
        if running { "RUNNING" } else { "finished" },
        s.str_field("app").unwrap_or("?"),
        mins,
        s.i64_field("steps").unwrap_or(0),
        s.i64_field("episodes").unwrap_or(0),
        s.i64_field("states").unwrap_or(0),
        s.i64_field("actions_tried").unwrap_or(0),
        s.i64_field("actions_seen").unwrap_or(0),
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
    // compiled knowledge from the wiki: how to launch it, what never to click, where to start
    let first = app.split_whitespace().next().unwrap_or("").to_string();
    let hints = if a.bool_field("use_wiki").unwrap_or(true) { wiki_hints(&first) } else { None };
    let launch_owned: String = match (&hints, app.split_whitespace().count()) {
        (Some(h), 1) => h.str_field("launch").unwrap_or(app).to_string(), // bare name -> wiki's launch line
        _ => app.to_string(),
    };
    let app: &str = &launch_owned;
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
    let mut deny: Vec<Value> = a.get("deny").and_then(|d| d.as_array()).cloned().unwrap_or_default();
    let mut wiki_note = String::from("no wiki page for this app yet");
    if let Some(h) = &hints {
        deny.extend(h.get("deny").and_then(|d| d.as_array()).cloned().unwrap_or_default());
        let seeds = h.get("seeds").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        wiki_note = format!("wiki: apps/{first}.md → launch '{app}', {} deny rules, {} seed paths", deny.len(), seeds.len());
        if !seeds.is_empty() { cfg.set("seeds", Value::Arr(seeds)); }
    }
    if !deny.is_empty() { cfg.set("deny", Value::Arr(deny)); }
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
                "exploration started: run={id}\napp: {app} · {minutes} min · sandboxed HOME: {}\n{wiki_note}\n\
                 No tokens are spent while it runs. Check later with explore__status / explore__findings, \
                 then explore__digest to ingest it into the wiki.",
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

/// Compact markdown digest of a run: the raw source the wiki ingests (cheap to read, never rewritten).
fn digest(a: &Value) -> ToolResult {
    let dir = match run_dir(a) { Ok(d) => d, Err(e) => return ToolResult::error(e) };
    let id = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let cfg = read_json(&dir.join("config.json")).unwrap_or(Value::Null);
    let st = read_json(&dir.join("status.json")).unwrap_or(Value::Null);
    if st.bool_field("running").unwrap_or(false) && pid_alive(st.i64_field("explorer_pid").unwrap_or(0)) {
        return ToolResult::error(format!("run {id} is still running; digest it when it finishes"));
    }
    let cov = read_json(&dir.join("coverage.json")).unwrap_or(Value::Null);
    let app = cfg.str_field("app").unwrap_or("?");
    let slug = app.split_whitespace().next().unwrap_or("app");
    let date = std::process::Command::new("date").arg("+%Y-%m-%d").output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let mins = (st.get("updated").and_then(|v| v.as_f64()).unwrap_or(0.0) - st.get("started").and_then(|v| v.as_f64()).unwrap_or(0.0)) / 60.0;
    let mut md = format!(
        "---\ntype: run\napp: {slug}\nlaunch: \"{app}\"\nrun: {id}\ndate: {date}\nminutes: {mins:.1}\nsteps: {}\nepisodes: {}\nstates: {}\nactions_tried: {}\nactions_seen: {}\nfindings: {}\nseed: {}\ngo_explore: {}\n---\n\n# Run {id} — {slug}\n\n",
        st.i64_field("steps").unwrap_or(0), st.i64_field("episodes").unwrap_or(0), st.i64_field("states").unwrap_or(0),
        st.i64_field("actions_tried").unwrap_or(0), st.i64_field("actions_seen").unwrap_or(0), st.i64_field("findings").unwrap_or(0),
        cfg.i64_field("seed").unwrap_or(0), cfg.bool_field("go_explore").unwrap_or(true));

    md.push_str("## Findings\n\n");
    let f = load_findings(&dir);
    if f.is_empty() { md.push_str("None.\n"); }
    for v in &f {
        let ev = v.str_field("evidence").unwrap_or("");
        let frames: Vec<&str> = ev.lines().filter(|l| l.trim_start().starts_with('#')).take(8).collect();
        let steps: Vec<String> = v.get("repro").and_then(|r| r.as_array()).into_iter().flatten()
            .map(|s| format!("{} {} \"{}\"", s.str_field("action").unwrap_or(""), s.str_field("role").unwrap_or(""), s.str_field("name").unwrap_or("")))
            .collect();
        let tail = if steps.len() > 12 { &steps[steps.len() - 12..] } else { &steps[..] };
        md.push_str(&format!("### {} `{}` ×{}\n- signature: `{}`\n- detail: {}\n- last steps ({} total): {}\n",
            v.str_field("kind").unwrap_or(""), v.str_field("id").unwrap_or(""), v.i64_field("count").unwrap_or(1),
            v.str_field("signature").unwrap_or(""), v.str_field("detail").unwrap_or(""), steps.len(), tail.join(" → ")));
        if !frames.is_empty() { md.push_str(&format!("- top frames:\n```\n{}\n```\n", frames.join("\n"))); }
        md.push('\n');
    }

    md.push_str("## Controls (how often each kind led somewhere new)\n\n| control | tries | novel |\n|---|---|---|\n");
    for r in cov.get("controls").and_then(|x| x.as_array()).into_iter().flatten() {
        md.push_str(&format!("| {} | {} | {:.1}% |\n", r.str_field("control").unwrap_or("?"),
            r.get("tries").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64, 100.0 * r.get("novelty_rate").and_then(|v| v.as_f64()).unwrap_or(0.0)));
    }
    md.push_str("\n## UI states (shortest known path)\n\n");
    let states = cov.get("states").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    for stv in states.iter().take(40) {
        let path: Vec<&str> = stv.get("path").and_then(|p| p.as_array()).into_iter().flatten().filter_map(|x| x.as_str()).collect();
        md.push_str(&format!("- `{}` depth {}: {}\n", stv.str_field("state").unwrap_or("?"), stv.i64_field("depth").unwrap_or(0),
            if path.is_empty() { "(start)".into() } else { truncate(&path.join(" → "), 240) }));
    }
    if states.len() > 40 { md.push_str(&format!("- … {} more states\n", states.len() - 40)); }
    let list = |k: &str, n: usize| cov.get(k).and_then(|x| x.as_array()).map(|a| a.iter().take(n).filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    md.push_str(&format!("\n## Most tried\n\n{}\n\n## Seen but never tried\n\n{}\n", list("most_tried", 12), list("never_tried", 60)));
    // deepest paths as ready-to-paste seeds for the app page's mcpos-explore block
    let mut deep: Vec<&Value> = states.iter().filter(|s| s.i64_field("depth").unwrap_or(0) >= 2).collect();
    deep.sort_by(|a, b| b.i64_field("depth").unwrap_or(0).cmp(&a.i64_field("depth").unwrap_or(0)));
    let seeds: Vec<Value> = deep.iter().take(5).filter_map(|s| s.get("path_steps").cloned()).collect();
    md.push_str(&format!("\n## Seed paths (deepest states; copy into apps/{slug}.md)\n\n```json\n{}\n```\n", Value::Arr(seeds).to_json()));

    let write = a.bool_field("write").unwrap_or(true);
    let mut where_ = String::from("(not written)");
    if write {
        let raw = wiki_dir().join("raw").join("runs");
        let _ = std::fs::create_dir_all(&raw);
        let p = raw.join(format!("{id}.md"));
        if p.exists() {
            where_ = format!("{} (already existed: raw sources are immutable, not rewritten)", p.display());
        } else if std::fs::write(&p, &md).is_ok() {
            where_ = p.display().to_string();
        }
    }
    ToolResult::text(format!("digest: {where_}\nNext: ingest it following ~/mcpos-wiki/CLAUDE.md (update apps/{slug}.md, bugs/, toolkits/, index.md, log.md).\n\n{md}"))
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
                ("use_wiki", "boolean", "Apply ~/mcpos-wiki/apps/<app>.md hints: launch line, deny rules, seed paths (default true)"),
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
        .tool("digest", "Compact markdown digest of a finished run, saved as an immutable raw source in ~/mcpos-wiki/raw/runs/. \
             This is what the wiki ingests (findings, top frames, controls, states, ready-to-paste seed paths).",
            schema(&[("run", "string", "Run id (latest if omitted)"), ("write", "boolean", "Save into the wiki (default true)")], &[]), digest)
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
