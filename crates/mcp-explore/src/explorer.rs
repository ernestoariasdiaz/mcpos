//! Token-free GUI explorer: novelty-driven (UCB) walk over an app's accessibility tree,
//! with an oracle (crash / hang / error logs) and de-duplicated findings with repro steps.

use mcp_core::json::{self, obj, Value};
use mcp_core::proc::{have_sudo, maybe_root, run, run_argv};
use mcp_core::util::{truncate, unix_now};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

pub const WORKER_PY: &str = include_str!("worker.py");

/// Actions we never trigger: destructive, or they end the session/app instead of exploring it.
const DENY: &[&str] = &[
    "delete", "trash", "remove", "erase", "shred", "format", "wipe", "empty", "purge",
    "borrar", "eliminar", "papelera", "vaciar", "suprimir",
    "shut down", "shutdown", "restart", "reboot", "log out", "logout", "sleep", "hibernate", "lock",
    "apagar", "reiniciar", "cerrar sesión", "suspender", "bloquear",
    "quit", "exit", "salir", "close window", "cerrar ventana",
];

const FUZZ: &[&str] = &[
    "", "a", "0", "-1", "99999999999999999999", "3.4e38", "../../../../etc/passwd",
    "%s%s%s%n%x", "ñandú 😀 日本語 مرحبا", "<b>x</b><script>alert(1)</script>", "\t\n\r",
    "' OR 1=1 --", "null", "file:///proc/self/maps",
];

pub fn fuzz_long() -> String { "A".repeat(5000) }

// ---------------------------------------------------------------- small utils

pub fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
    format!("{h:016x}")
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x }
    fn below(&mut self, n: usize) -> usize { (self.next() % n.max(1) as u64) as usize }
    fn unit(&mut self) -> f64 { (self.next() >> 11) as f64 / (1u64 << 53) as f64 }
}

fn denied(name: &str, extra: &[String]) -> bool {
    let n = name.to_lowercase();
    DENY.iter().any(|d| n.contains(d)) || extra.iter().any(|d| !d.is_empty() && n.contains(&d.to_lowercase()))
}

/// Function names of the top frames of a gdb backtrace, skipping libc/abort plumbing.
pub fn stack_signature(bt: &str, frames: usize) -> String {
    const SKIP: &[&str] = &["raise", "abort", "__pthread_kill", "pthread_kill", "__GI_", "_start",
        "__libc_start", "__restore_rt", "??", "g_assertion_message", "qt_message_fatal", "qFatal",
        "__assert_fail", "__kernel_vsyscall", "syscall", "__internal_syscall"];
    let mut out = Vec::new();
    for line in bt.lines() {
        let l = line.trim_start();
        if !l.starts_with('#') { continue; }
        // "#3  0x00007f.. in Foo::bar (this=...) at x.cpp:12"  or  "#0  main () at a.c:3"
        let after = match l.find(" in ") { Some(i) => &l[i + 4..], None => l.split_once(char::is_whitespace).map(|x| x.1.trim_start()).unwrap_or("") };
        let func = after.split(" (").next().unwrap_or("").trim();
        if func.is_empty() || SKIP.iter().any(|s| func.starts_with(s)) { continue; }
        if out.last().map(|x: &String| x == func).unwrap_or(false) { continue; }
        out.push(func.to_string());
        if out.len() >= frames { break; }
    }
    out.join(" <- ")
}

/// Normalize a log line so the same error with different numbers/addresses dedups.
pub fn norm_log(s: &str) -> String {
    let mut out = String::new();
    let mut prev_digit = false;
    let mut chars = s.trim().chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && chars.peek() == Some(&'x') {
            chars.next();
            while chars.peek().map(|c| c.is_ascii_hexdigit()).unwrap_or(false) { chars.next(); }
            out.push_str("0x#"); prev_digit = false; continue;
        }
        if c.is_ascii_digit() { if !prev_digit { out.push('#'); } prev_digit = true; continue; }
        prev_digit = false;
        out.push(c);
    }
    out.chars().take(160).collect()
}

// ---------------------------------------------------------------- AT-SPI worker

pub enum WErr { Timeout, Dead(String) }

pub struct Worker {
    script: PathBuf,
    child: Option<(Child, ChildStdin, Receiver<String>)>,
}

impl Worker {
    pub fn new(dir: &Path) -> Result<Self, String> {
        let script = dir.join("worker.py");
        std::fs::write(&script, WORKER_PY).map_err(|e| format!("write worker: {e}"))?;
        let mut w = Worker { script, child: None };
        w.start()?;
        Ok(w)
    }
    fn start(&mut self) -> Result<(), String> {
        self.kill();
        let python = std::env::var("MCP_EXPLORE_PYTHON").unwrap_or_else(|_| "python3".into());
        let mut c = Command::new(&python)
            .arg("-I").arg(&self.script)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
            .spawn().map_err(|e| format!("spawn {python}: {e}"))?;
        let stdin = c.stdin.take().unwrap();
        let stdout = c.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(l) = line else { break };
                if tx.send(l).is_err() { break; }
            }
        });
        self.child = Some((c, stdin, rx));
        match self.req(&obj([("cmd", "ping".into())]), Duration::from_secs(20)) {
            Ok(v) if v.str_field("fatal").is_some() => Err(v.str_field("fatal").unwrap().to_string()),
            Ok(_) => Ok(()),
            Err(WErr::Timeout) => Err("worker did not start".into()),
            Err(WErr::Dead(e)) => Err(e),
        }
    }
    pub fn restart(&mut self) -> Result<(), String> { self.start() }
    pub fn kill(&mut self) {
        if let Some((mut c, _, _)) = self.child.take() { let _ = c.kill(); let _ = c.wait(); }
    }
    pub fn req(&mut self, v: &Value, timeout: Duration) -> Result<Value, WErr> {
        let Some((_, stdin, rx)) = self.child.as_mut() else { return Err(WErr::Dead("no worker".into())) };
        if writeln!(stdin, "{}", v.to_json()).and_then(|_| stdin.flush()).is_err() {
            return Err(WErr::Dead("worker stdin closed".into()));
        }
        match rx.recv_timeout(timeout) {
            Ok(line) => json::parse(&line).map_err(|e| WErr::Dead(format!("bad worker reply: {e}"))),
            Err(RecvTimeoutError::Timeout) => Err(WErr::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(WErr::Dead("worker exited".into())),
        }
    }
}

impl Drop for Worker { fn drop(&mut self) { self.kill(); } }

// ---------------------------------------------------------------- app under test

pub struct App { pub child: Child, pub pid: u32, pub started: f64 }

pub fn launch(cmd: &str, home: Option<&Path>) -> Result<App, String> {
    let mut c = Command::new("sh");
    c.arg("-c").arg(format!("exec {cmd}"))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .process_group(0);
    if let Some(h) = home {
        let _ = std::fs::remove_dir_all(h);
        for sub in ["", ".config", ".local/share", ".cache", ".local/state", "Documents"] {
            let _ = std::fs::create_dir_all(h.join(sub));
        }
        let _ = std::fs::write(h.join("Documents/sample.txt"), "mcpOS explorer sample file\nline 2\n");
        c.env("HOME", h)
            .env("XDG_CONFIG_HOME", h.join(".config"))
            .env("XDG_DATA_HOME", h.join(".local/share"))
            .env("XDG_CACHE_HOME", h.join(".cache"))
            .env("XDG_STATE_HOME", h.join(".local/state"));
    }
    let child = c.spawn().map_err(|e| format!("launch {cmd}: {e}"))?;
    let pid = child.id();
    Ok(App { child, pid, started: unix_now() })
}

pub fn kill_app(app: &mut App) {
    let pg = format!("-{}", app.pid);
    let _ = run("kill", &["-TERM", "--", &pg], None, Duration::from_secs(5));
    for _ in 0..20 {
        if matches!(app.child.try_wait(), Ok(Some(_))) { return; }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = run("kill", &["-KILL", "--", &pg], None, Duration::from_secs(5));
    let _ = app.child.kill();
    let _ = app.child.wait();
}

// ---------------------------------------------------------------- oracle

fn sysrun(argv: &[&str], secs: u64) -> String {
    let full = maybe_root(have_sudo(), argv);
    run_argv(&full, None, Duration::from_secs(secs)).map(|o| format!("{}{}", o.stdout, o.stderr)).unwrap_or_default()
}

/// Backtrace of a crashed pid from systemd-coredump, with debuginfod symbols.
pub fn crash_backtrace(pid: u32, wait_secs: u64) -> Option<String> {
    let p = pid.to_string();
    let deadline = Instant::now() + Duration::from_secs(wait_secs);
    loop {
        let info = sysrun(&["coredumpctl", "info", "--no-pager", &p], 20);
        if info.contains("Signal:") || info.contains("PID:") { break; }
        if Instant::now() >= deadline { return None; }
        std::thread::sleep(Duration::from_millis(500));
    }
    let urls = std::env::var("DEBUGINFOD_URLS").unwrap_or_else(|_| "https://debuginfod.archlinux.org".into());
    let env_kv = format!("DEBUGINFOD_URLS={urls}");
    let bt = sysrun(&["env", &env_kv, "coredumpctl", "debug", "--no-pager", &p,
        "--debugger-arguments=-batch -iex 'set debuginfod enabled on' -ex 'bt 40'"], 600);
    Some(bt)
}

/// Live backtrace of a hung process (all threads).
pub fn hang_backtrace(pid: u32) -> String {
    let p = pid.to_string();
    let urls = std::env::var("DEBUGINFOD_URLS").unwrap_or_else(|_| "https://debuginfod.archlinux.org".into());
    let env_kv = format!("DEBUGINFOD_URLS={urls}");
    sysrun(&["env", &env_kv, "gdb", "-p", &p, "-batch", "-iex", "set debuginfod enabled on",
        "-ex", "thread apply all bt 25"], 300)
}

pub fn journal_errors(pid: u32, since: f64) -> Vec<String> {
    let since = format!("--since=@{}", since as u64);
    let pidf = format!("_PID={pid}");
    sysrun(&["journalctl", "--no-pager", "-o", "cat", "-p", "err", &since, &pidf], 30)
        .lines().map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with("-- No entries"))
        .collect()
}

// ---------------------------------------------------------------- run state on disk

pub struct Run {
    pub dir: PathBuf,
    pub cfg: Value,
    findings: HashMap<String, Value>,
    pub steps: u64,
    pub episodes: u64,
    pub states: HashSet<String>,
    last_event: String,
    started: f64,
}

impl Run {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let cfg = std::fs::read_to_string(dir.join("config.json")).map_err(|e| format!("config: {e}"))?;
        let cfg = json::parse(&cfg).map_err(|e| format!("config: {e}"))?;
        let mut findings = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(dir.join("findings")) {
            for e in rd.flatten() {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    if let Ok(v) = json::parse(&t) {
                        if let Some(id) = v.str_field("id") { findings.insert(id.to_string(), v); }
                    }
                }
            }
        }
        Ok(Run { dir: dir.to_path_buf(), cfg, findings, steps: 0, episodes: 0, states: HashSet::new(),
                 last_event: "starting".into(), started: unix_now() })
    }

    fn event(&mut self, kind: &str, msg: &str) {
        self.last_event = format!("{kind}: {msg}");
        let line = obj([("ts", unix_now().into()), ("kind", kind.into()), ("msg", msg.into())]);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("events.jsonl")) {
            let _ = writeln!(f, "{}", line.to_json());
        }
    }

    pub fn write_status(&self, running: bool) {
        let s = obj([
            ("running", running.into()),
            ("explorer_pid", (std::process::id() as u64).into()),
            ("app", self.cfg.str_field("app").unwrap_or("").into()),
            ("started", self.started.into()),
            ("updated", unix_now().into()),
            ("steps", self.steps.into()),
            ("episodes", self.episodes.into()),
            ("states", self.states.len().into()),
            ("findings", self.findings.len().into()),
            ("last_event", self.last_event.as_str().into()),
        ]);
        let tmp = self.dir.join("status.json.tmp");
        if std::fs::write(&tmp, s.to_json()).is_ok() { let _ = std::fs::rename(tmp, self.dir.join("status.json")); }
    }

    /// Record (or bump) a finding; returns true if it's new.
    fn finding(&mut self, kind: &str, signature: &str, detail: &str, repro: &[Value], extra: &str) -> bool {
        let id = fnv(&format!("{kind}|{}|{signature}", self.cfg.str_field("app").unwrap_or("")))[..12].to_string();
        let now = unix_now();
        let is_new = !self.findings.contains_key(&id);
        let v = if let Some(old) = self.findings.get(&id) {
            let mut v = old.clone();
            v.set("count", (old.i64_field("count").unwrap_or(1) + 1).into());
            v.set("last_seen", now.into());
            v
        } else {
            obj([
                ("id", id.as_str().into()),
                ("kind", kind.into()),
                ("app", self.cfg.str_field("app").unwrap_or("").into()),
                ("signature", signature.into()),
                ("detail", truncate(detail, 300).into()),
                ("count", 1i64.into()),
                ("first_seen", now.into()),
                ("last_seen", now.into()),
                ("episode", self.episodes.into()),
                ("repro", Value::Arr(repro.to_vec())),
                ("evidence", truncate(extra, 12_000).into()),
            ])
        };
        let _ = std::fs::create_dir_all(self.dir.join("findings"));
        let _ = std::fs::write(self.dir.join("findings").join(format!("{id}.json")), v.to_json());
        self.findings.insert(id.clone(), v);
        self.event("finding", &format!("{kind} {id} {}", truncate(signature, 120)));
        is_new
    }

    fn stop_requested(&self) -> bool { self.dir.join("STOP").exists() }
}

// ---------------------------------------------------------------- the explorer

struct Cand { path: String, key: String, role: String, name: String, action: String, text: Option<String> }

fn candidates(snap: &Value, deny: &[String], rng: &mut Rng) -> Vec<Cand> {
    let mut out = Vec::new();
    for a in snap.get("actions").and_then(|x| x.as_array()).into_iter().flatten() {
        let path = a.str_field("path").unwrap_or("").to_string();
        let role = a.str_field("role").unwrap_or("").to_string();
        let name = a.str_field("name").unwrap_or("").to_string();
        if denied(&name, deny) { continue; }
        for act in a.get("acts").and_then(|x| x.as_array()).into_iter().flatten().filter_map(|x| x.as_str()) {
            if denied(act, deny) { continue; }
            out.push(Cand { key: format!("{role}|{name}|{act}"), path: path.clone(), role: role.clone(),
                            name: name.clone(), action: act.to_string(), text: None });
        }
        if a.bool_field("editable").unwrap_or(false) {
            let pick = rng.below(FUZZ.len() + 1);
            let text = if pick == FUZZ.len() { fuzz_long() } else { FUZZ[pick].to_string() };
            out.push(Cand { key: format!("{role}|{name}|type"), path: path.clone(), role: role.clone(),
                            name: name.clone(), action: "type".into(), text: Some(text) });
        }
    }
    out
}

fn step_value(c: &Cand) -> Value {
    let mut v = obj([("path", c.path.as_str().into()), ("role", c.role.as_str().into()),
                     ("name", c.name.as_str().into()), ("action", c.action.as_str().into())]);
    if let Some(t) = &c.text { v.set("text", truncate(t, 200).into()); }
    v
}

enum EpisodeEnd { Normal, Crashed, Hung, Exited, Stop }

/// Re-apply one recorded step on the current UI, matching the element by role+name (paths shift).
/// Ok(false) = element not present now.
fn apply_step(worker: &mut Worker, pid: u64, s: &Value) -> Result<bool, WErr> {
    let snap = worker.req(&obj([("cmd", "snapshot".into()), ("pid", pid.into())]), Duration::from_secs(15))?;
    let (role, name, action) = (s.str_field("role").unwrap_or(""), s.str_field("name").unwrap_or(""), s.str_field("action").unwrap_or(""));
    let Some(path) = snap.get("actions").and_then(|a| a.as_array()).and_then(|a| {
        a.iter().find(|x| x.str_field("role") == Some(role) && x.str_field("name") == Some(name))
            .and_then(|x| x.str_field("path").map(String::from))
    }) else { return Ok(false) };
    let req = if action == "type" {
        let text = s.str_field("text").unwrap_or("").to_string();
        let text = if text.starts_with("AAAAAAAA") { fuzz_long() } else { text };
        obj([("cmd", "type".into()), ("pid", pid.into()), ("path", path.as_str().into()), ("text", text.into())])
    } else {
        obj([("cmd", "act".into()), ("pid", pid.into()), ("path", path.as_str().into()), ("action", action.into())])
    };
    worker.req(&req, Duration::from_secs(15)).map(|_| true)
}

/// Prior belief that an action on this kind of control opens something new.
fn role_prior(role: &str, action: &str) -> f64 {
    if action == "type" { return 0.6; }
    match role {
        "menu" | "menu item" | "check menu item" | "radio menu item" | "page tab" | "combo box"
        | "toggle button" | "check box" | "radio button" | "tree item" | "link" | "list item" | "menu bar" => 1.0,
        "push button" | "button" => 0.5,
        _ => 0.4,
    }
}

/// Run the exploration until the deadline / STOP file. Status and findings are written under `dir`.
pub fn explore(dir: &Path) -> Result<(), String> {
    let mut r = Run::load(dir)?;
    let app_cmd = r.cfg.str_field("app").ok_or("config: missing app")?.to_string();
    let minutes = r.cfg.get("minutes").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let max_ep_steps = r.cfg.i64_field("max_steps_per_episode").unwrap_or(50).max(1) as u64;
    let delay = Duration::from_millis(r.cfg.i64_field("step_delay_ms").unwrap_or(400).max(50) as u64);
    let sandbox = r.cfg.bool_field("sandbox_home").unwrap_or(true);
    let deny: Vec<String> = r.cfg.get("deny").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let mut rng = Rng(r.cfg.i64_field("seed").unwrap_or(0x5eed) as u64 | 1);
    let deadline = Instant::now() + Duration::from_secs_f64(minutes * 60.0);
    let home = dir.join("home");

    let mut worker = Worker::new(dir)?;
    // UCB statistics keyed by (state, action key)
    let mut n_sa: HashMap<(String, String), (f64, f64)> = HashMap::new();
    let mut n_s: HashMap<String, f64> = HashMap::new();
    // Go-Explore archive: state -> (shortest known path from a fresh start, times chosen as start cell)
    let mut archive: HashMap<String, (Vec<Value>, f64)> = HashMap::new();
    // what each kind of control tends to do in *this* app: role|action -> (tries, novel outcomes)
    let mut role_stats: HashMap<String, (f64, f64)> = HashMap::new();
    let go_explore = r.cfg.bool_field("go_explore").unwrap_or(true);
    let mut tried_keys: HashMap<String, u64> = HashMap::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    r.event("start", &format!("app={app_cmd} minutes={minutes} go_explore={go_explore}"));
    r.write_status(true);

    'run: while Instant::now() < deadline && !r.stop_requested() {
        r.episodes += 1;
        let mut app = launch(&app_cmd, if sandbox { Some(&home) } else { None })?;
        let mut repro: Vec<Value> = Vec::new();
        let mut prev: Option<(String, String)> = None; // (state, key) of the last action
        let mut not_found = 0;

        // wait for the app to show up on the accessibility bus
        let mut up = false;
        for _ in 0..40 {
            if let Ok(Some(st)) = app.child.try_wait() {
                let early = unix_now() - app.started < 5.0;
                if early && st.success() {
                    r.event("error", "app exited immediately with status 0: it probably handed off to an \
                        already running instance (single-instance app). Close it or use a 'new instance' flag, e.g. 'kate -n'.");
                    r.write_status(false);
                    return Err("app handed off to an existing instance".into());
                }
                break;
            }
            match worker.req(&obj([("cmd", "snapshot".into()), ("pid", (app.pid as u64).into())]), Duration::from_secs(15)) {
                Ok(v) if v.bool_field("found") == Some(true) => { up = true; break; }
                Ok(_) => {}
                Err(_) => { let _ = worker.restart(); }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if !up && matches!(app.child.try_wait(), Ok(None)) {
            r.event("warn", "app never appeared on the AT-SPI bus (accessibility off for this toolkit?)");
        }

        let mut end = EpisodeEnd::Normal;
        // Go-Explore: usually go back to a rarely-chosen known state, then explore from there
        if go_explore && up && !archive.is_empty() && rng.unit() < 0.85 {
            let keys: Vec<&String> = archive.keys().collect();
            let weights: Vec<f64> = keys.iter().map(|k| 1.0 / (1.0 + archive[*k].1).sqrt()).collect();
            let total: f64 = weights.iter().sum();
            let mut pick = rng.unit() * total;
            let mut chosen = keys[0].clone();
            for (k, w) in keys.iter().zip(&weights) { if pick <= *w { chosen = (*k).clone(); break; } pick -= w; }
            let path = archive[&chosen].0.clone();
            archive.get_mut(&chosen).unwrap().1 += 1.0;
            for st in &path {
                match apply_step(&mut worker, app.pid as u64, st) {
                    Ok(true) => { repro.push(st.clone()); r.steps += 1; std::thread::sleep(delay); }
                    Ok(false) => break, // UI differs this time: explore from wherever we are
                    Err(WErr::Timeout) => { end = EpisodeEnd::Hung; break; }
                    Err(WErr::Dead(_)) => { let _ = worker.restart(); break; }
                }
                if let Ok(Some(_)) = app.child.try_wait() { end = EpisodeEnd::Exited; break; }
            }
        }
        let budget = if matches!(end, EpisodeEnd::Normal) { max_ep_steps } else { 0 };
        for _ in 0..budget {
            if r.stop_requested() || Instant::now() >= deadline { end = EpisodeEnd::Stop; break; }
            if let Ok(Some(_)) = app.child.try_wait() { end = EpisodeEnd::Exited; break; }

            let snap = match worker.req(&obj([("cmd", "snapshot".into()), ("pid", (app.pid as u64).into())]), Duration::from_secs(15)) {
                Ok(v) => v,
                Err(WErr::Timeout) => { end = EpisodeEnd::Hung; break; }
                Err(WErr::Dead(e)) => { r.event("warn", &format!("worker died: {e}")); let _ = worker.restart(); continue; }
            };
            if snap.bool_field("found") != Some(true) {
                not_found += 1;
                if not_found >= 4 { break; }
                std::thread::sleep(delay);
                continue;
            }
            not_found = 0;
            let state = snap.str_field("hash").unwrap_or("").to_string();
            let is_new = r.states.insert(state.clone());
            if let Some((ps, pk)) = prev.take() {
                let changed = ps != state;
                let e = n_sa.entry((ps, pk.clone())).or_insert((0.0, 0.0));
                e.1 += if is_new { 1.0 } else { 0.0 };
                let rk = role_key(&pk);
                let re = role_stats.entry(rk).or_insert((0.0, 0.0));
                re.0 += 1.0;
                re.1 += if is_new { 1.0 } else if changed { 0.25 } else { 0.0 };
            }
            match archive.get_mut(&state) {
                None => { archive.insert(state.clone(), (repro.clone(), 0.0)); }
                Some(c) if repro.len() < c.0.len() => c.0 = repro.clone(),
                _ => {}
            }
            *n_s.entry(state.clone()).or_insert(0.0) += 1.0;

            let cands = candidates(&snap, &deny, &mut rng);
            if cands.is_empty() { r.event("info", "no actionable elements"); break; }
            // UCB1 over novelty reward; untried actions first (random tie-break)
            let ns = n_s[&state];
            let mut best = 0usize;
            let mut best_score = f64::MIN;
            for (i, c) in cands.iter().enumerate() {
                seen_keys.insert(c.key.clone());
                // smoothed estimate of how often this kind of control leads somewhere new in this app
                let (rn, rw) = role_stats.get(&role_key(&c.key)).copied().unwrap_or((0.0, 0.0));
                let role_mean = (rw + 2.0 * role_prior(&c.role, &c.action)) / (rn + 2.0);
                let (n, w) = n_sa.get(&(state.clone(), c.key.clone())).copied().unwrap_or((0.0, 0.0));
                let score = (w + role_mean) / (n + 1.0) + 1.0 * ((ns + 1.0).ln() / (n + 1.0)).sqrt() + rng.unit() * 0.05;
                if score > best_score { best_score = score; best = i; }
            }
            let c = &cands[best];
            *tried_keys.entry(c.key.clone()).or_insert(0) += 1;
            n_sa.entry((state.clone(), c.key.clone())).or_insert((0.0, 0.0)).0 += 1.0;
            prev = Some((state.clone(), c.key.clone()));
            repro.push(step_value(c));

            let req = match &c.text {
                Some(t) => obj([("cmd", "type".into()), ("pid", (app.pid as u64).into()), ("path", c.path.as_str().into()), ("text", t.as_str().into())]),
                None => obj([("cmd", "act".into()), ("pid", (app.pid as u64).into()), ("path", c.path.as_str().into()), ("action", c.action.as_str().into())]),
            };
            match worker.req(&req, Duration::from_secs(15)) {
                Ok(_) => {}
                Err(WErr::Timeout) => { end = EpisodeEnd::Hung; break; }
                Err(WErr::Dead(e)) => { r.event("warn", &format!("worker died: {e}")); let _ = worker.restart(); }
            }
            r.steps += 1;
            if r.steps % 5 == 0 { r.write_status(true); }
            std::thread::sleep(delay);
            if let Ok(Some(_)) = app.child.try_wait() { end = EpisodeEnd::Exited; break; }
        }

        // ---- oracle
        match end {
            EpisodeEnd::Hung => {
                r.event("hang", &format!("pid {} stopped answering AT-SPI", app.pid));
                let bt = hang_backtrace(app.pid);
                let sig = { let s = stack_signature(&bt, 5); if s.is_empty() { "unresponsive (no symbols)".into() } else { s } };
                r.finding("hang", &sig, "app stopped answering accessibility requests for 15s", &repro, &bt);
                kill_app(&mut app);
                let _ = worker.restart();
            }
            _ => {
                let status = app.child.try_wait().ok().flatten();
                if let Some(st) = status {
                    if let Some(sig) = st.signal() {
                        end = EpisodeEnd::Crashed;
                        r.event("crash", &format!("pid {} killed by signal {sig}", app.pid));
                        let bt = crash_backtrace(app.pid, 20).unwrap_or_default();
                        let s = stack_signature(&bt, 5);
                        let sigtxt = if s.is_empty() { format!("signal {sig} (no core/symbols)") } else { s };
                        r.finding("crash", &sigtxt, &format!("terminated by signal {sig}"), &repro, &bt);
                    } else if st.code().unwrap_or(0) != 0 {
                        let code = st.code().unwrap_or(-1);
                        r.finding("exit", &format!("exit code {code}"), &format!("app exited with status {code}"), &repro, "");
                    }
                }
                if matches!(app.child.try_wait(), Ok(None)) { kill_app(&mut app); }
            }
        }
        for line in journal_errors(app.pid, app.started) {
            let sig = norm_log(&line);
            r.finding("log-error", &sig, &line, &repro, &line);
        }
        r.write_status(true);
        write_coverage(dir, &archive, &role_stats, &tried_keys, &seen_keys);
        if matches!(end, EpisodeEnd::Stop) { break 'run; }
    }
    r.event("done", &format!("{} steps, {} episodes, {} states, {} findings", r.steps, r.episodes, r.states.len(), r.findings.len()));
    r.write_status(false);
    Ok(())
}

fn role_key(action_key: &str) -> String {
    // "role|name|action" -> "role|action"
    let mut it = action_key.split('|');
    let role = it.next().unwrap_or("");
    let action = action_key.rsplit('|').next().unwrap_or("");
    format!("{role}|{action}")
}

fn write_coverage(dir: &Path, archive: &HashMap<String, (Vec<Value>, f64)>, role_stats: &HashMap<String, (f64, f64)>,
                  tried: &HashMap<String, u64>, seen: &HashSet<String>) {
    let mut states: Vec<(&String, &(Vec<Value>, f64))> = archive.iter().collect();
    states.sort_by_key(|(_, (p, _))| p.len());
    let states: Vec<Value> = states.iter().map(|(h, (p, chosen))| obj([
        ("state", h.as_str().into()),
        ("depth", p.len().into()),
        ("chosen", (*chosen).into()),
        ("path", Value::Arr(p.iter().map(|st| Value::Str(format!("{} {} {:?}",
            st.str_field("action").unwrap_or(""), st.str_field("role").unwrap_or(""), st.str_field("name").unwrap_or("")))).collect())),
    ])).collect();
    let mut roles: Vec<(&String, &(f64, f64))> = role_stats.iter().collect();
    roles.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap_or(std::cmp::Ordering::Equal));
    let roles: Vec<Value> = roles.iter().map(|(k, (n, w))| obj([
        ("control", k.as_str().into()), ("tries", (*n).into()), ("novelty_rate", (if *n > 0.0 { w / n } else { 0.0 }).into()),
    ])).collect();
    let mut never: Vec<&String> = seen.iter().filter(|k| !tried.contains_key(*k)).collect();
    never.sort();
    let mut top: Vec<(&String, &u64)> = tried.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1));
    let v = obj([
        ("states", Value::Arr(states)),
        ("controls", Value::Arr(roles)),
        ("most_tried", Value::Arr(top.iter().take(15).map(|(k, n)| Value::Str(format!("{n}× {k}"))).collect())),
        ("never_tried", Value::Arr(never.iter().take(60).map(|k| Value::Str((*k).clone())).collect())),
        ("distinct_actions_seen", seen.len().into()),
        ("distinct_actions_tried", tried.len().into()),
    ]);
    let _ = std::fs::write(dir.join("coverage.json"), v.to_json());
}

/// Replay a finding's repro steps (matched by role+name+action, falling back to path) and
/// report whether the same kind of failure happened again.
pub fn replay(dir: &Path, finding: &Value, delay: Duration) -> Result<String, String> {
    let r = Run::load(dir)?;
    let app_cmd = r.cfg.str_field("app").ok_or("config: missing app")?.to_string();
    let home = dir.join("home-replay");
    let mut worker = Worker::new(dir)?;
    let mut app = launch(&app_cmd, Some(&home))?;
    let pid_n = app.pid as u64;
    let pid = || -> Value { pid_n.into() };
    for _ in 0..40 {
        if let Ok(v) = worker.req(&obj([("cmd", "snapshot".into()), ("pid", pid())]), Duration::from_secs(15)) {
            if v.bool_field("found") == Some(true) { break; }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let steps = finding.get("repro").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let mut log = Vec::new();
    let mut outcome = "no failure reproduced".to_string();
    for (i, s) in steps.iter().enumerate() {
        if let Ok(Some(st)) = app.child.try_wait() {
            outcome = match st.signal() { Some(sig) => format!("CRASH reproduced (signal {sig}) after {i} steps"),
                                          None => format!("app exited ({:?}) after {i} steps", st.code()) };
            break;
        }
        let snap = match worker.req(&obj([("cmd", "snapshot".into()), ("pid", pid())]), Duration::from_secs(15)) {
            Ok(v) => v,
            Err(WErr::Timeout) => { outcome = format!("HANG reproduced at step {i}"); break; }
            Err(WErr::Dead(e)) => return Err(e),
        };
        let (role, name, action) = (s.str_field("role").unwrap_or(""), s.str_field("name").unwrap_or(""), s.str_field("action").unwrap_or(""));
        let path = snap.get("actions").and_then(|a| a.as_array()).and_then(|a| {
            a.iter().find(|x| x.str_field("role") == Some(role) && x.str_field("name") == Some(name))
                .and_then(|x| x.str_field("path").map(String::from))
        }).or_else(|| s.str_field("path").map(String::from)).unwrap_or_default();
        let req = if action == "type" {
            let text = s.str_field("text").unwrap_or("").to_string();
            let text = if text.starts_with("AAAAAAAA") { fuzz_long() } else { text };
            obj([("cmd", "type".into()), ("pid", pid()), ("path", path.as_str().into()), ("text", text.into())])
        } else {
            obj([("cmd", "act".into()), ("pid", pid()), ("path", path.as_str().into()), ("action", action.into())])
        };
        log.push(format!("{i}: {action} {role} {name:?}"));
        if let Err(WErr::Timeout) = worker.req(&req, Duration::from_secs(15)) { outcome = format!("HANG reproduced at step {i}"); break; }
        std::thread::sleep(delay);
    }
    if outcome == "no failure reproduced" {
        // the last step may be the one that breaks things: check crash, then hang, then logs
        std::thread::sleep(Duration::from_secs(2));
        if let Ok(Some(st)) = app.child.try_wait() {
            if let Some(sig) = st.signal() { outcome = format!("CRASH reproduced (signal {sig}) after the last step"); }
        } else if let Err(WErr::Timeout) = worker.req(&obj([("cmd", "snapshot".into()), ("pid", pid())]), Duration::from_secs(15)) {
            outcome = "HANG reproduced after the last step".into();
        }
    }
    if outcome == "no failure reproduced" && finding.str_field("kind") == Some("log-error") {
        let want = finding.str_field("signature").unwrap_or("");
        if journal_errors(app.pid, app.started).iter().any(|l| norm_log(l) == want) {
            outcome = "LOG-ERROR reproduced (same normalized message in the journal)".into();
        }
    }
    if matches!(app.child.try_wait(), Ok(None)) { kill_app(&mut app); }
    Ok(format!("{outcome}\n--- steps replayed ---\n{}", log.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signature() {
        let bt = "#0  __pthread_kill_implementation (threadid=1) at pthread_kill.c:44\n\
                  #1  0x00007f in raise (sig=11) at raise.c:26\n\
                  #2  0x00005 in KateViewInternal::paintEvent (this=0x1) at view.cpp:9\n\
                  #3  0x00005 in QWidget::event (this=0x2) at qwidget.cpp:1\n\
                  #4  0x00005 in ?? ()\n\
                  #5  main () at main.cpp:3";
        assert_eq!(stack_signature(bt, 5), "KateViewInternal::paintEvent <- QWidget::event <- main");
    }
    #[test]
    fn logs() {
        assert_eq!(norm_log("QFile 0x55d3a1 failed after 1234 ms on fd 7"), "QFile 0x# failed after # ms on fd #");
    }
    #[test]
    fn deny() {
        assert!(denied("Move to Trash", &[]));
        assert!(denied("Cerrar sesión", &[]));
        assert!(!denied("Open File…", &[]));
        assert!(denied("Print", &["print".into()]));
    }
}
