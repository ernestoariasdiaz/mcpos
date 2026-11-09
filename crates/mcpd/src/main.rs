//! mcpd: the mcpOS gateway. Spawns every MCP server listed in servers.conf, aggregates their
//! tools under `<server>__<tool>` names, routes calls, and writes an audit log of every call.
//!
//! Config lookup: $MCPD_CONFIG, ~/.config/mcpd/servers.conf, /etc/mcpd/servers.conf
//! Format:   name = /path/to/server [args...]      (# comments)

use mcp_core::json::{self, obj, Value};
use mcp_core::util::{schema, unix_now};
use mcp_core::{serve_stdio, Handler, ToolDef, ToolResult};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

const SEP: &str = "__";

struct Server {
    name: String,
    argv: Vec<String>,
    proc: Option<(Child, ChildStdin, BufReader<ChildStdout>)>,
    next_id: i64,
    tools: Vec<ToolDef>,
    last_error: Option<String>,
}

impl Server {
    fn new(name: String, argv: Vec<String>) -> Self {
        Server { name, argv, proc: None, next_id: 1, tools: vec![], last_error: None }
    }

    fn start(&mut self) -> Result<(), String> {
        self.stop();
        let mut child = Command::new(&self.argv[0])
            .args(&self.argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // child logs land in mcpd's stderr / journal
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", self.argv[0]))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        self.proc = Some((child, stdin, stdout));

        self.request(
            "initialize",
            obj([
                ("protocolVersion", mcp_core::server::DEFAULT_PROTOCOL.into()),
                ("capabilities", Value::Obj(vec![])),
                ("clientInfo", obj([("name", "mcpd".into()), ("version", env!("CARGO_PKG_VERSION").into())])),
            ]),
        )?;
        self.notify("notifications/initialized")?;
        let r = self.request("tools/list", Value::Obj(vec![]))?;
        self.tools = r
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|ts| {
                ts.iter()
                    .filter_map(|t| {
                        Some(ToolDef {
                            name: t.str_field("name")?.to_string(),
                            description: t.str_field("description").unwrap_or("").to_string(),
                            input_schema: t.get("inputSchema").cloned().unwrap_or_else(|| schema(&[], &[])),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.last_error = None;
        eprintln!("mcpd: {} up, {} tools", self.name, self.tools.len());
        Ok(())
    }

    fn stop(&mut self) {
        if let Some((mut c, _, _)) = self.proc.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn alive(&mut self) -> bool {
        match &mut self.proc {
            Some((c, _, _)) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    fn write(&mut self, v: &Value) -> Result<(), String> {
        let (_, stdin, _) = self.proc.as_mut().ok_or("not running")?;
        writeln!(stdin, "{}", v.to_json()).and_then(|_| stdin.flush()).map_err(|e| format!("{}: write: {e}", self.name))
    }

    fn notify(&mut self, method: &str) -> Result<(), String> {
        self.write(&obj([("jsonrpc", "2.0".into()), ("method", method.into())]))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&obj([
            ("jsonrpc", "2.0".into()),
            ("id", id.into()),
            ("method", method.into()),
            ("params", params),
        ]))?;
        let name = self.name.clone();
        let (_, _, stdout) = self.proc.as_mut().ok_or("not running")?;
        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line).map_err(|e| format!("{name}: read: {e}"))?;
            if n == 0 {
                return Err(format!("{name}: server exited"));
            }
            let Ok(msg) = json::parse(line.trim()) else { continue };
            if msg.i64_field("id") != Some(id) || msg.get("method").is_some() {
                continue; // notification or server->client request: ignored for now
            }
            if let Some(e) = msg.get("error") {
                return Err(format!("{name}: {}", e.str_field("message").unwrap_or("error")));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

struct Gateway {
    servers: Vec<Server>,
    audit: Option<std::fs::File>,
}

impl Gateway {
    fn audit(&mut self, tool: &str, args: &Value, r: &ToolResult, ms: u128) {
        let line = obj([
            ("ts", unix_now().into()),
            ("tool", tool.into()),
            ("args", args.clone()),
            ("is_error", r.is_error.into()),
            ("ms", (ms as u64).into()),
        ]);
        eprintln!("mcpd: call {tool} -> {} in {ms} ms", if r.is_error { "ERROR" } else { "ok" });
        if let Some(f) = &mut self.audit {
            let _ = writeln!(f, "{}", line.to_json());
        }
    }

    fn status(&mut self) -> ToolResult {
        let mut s = String::new();
        for srv in &mut self.servers {
            let alive = srv.alive();
            s.push_str(&format!(
                "{}: {} — {} tools — {}{}\n",
                srv.name,
                if alive { "running" } else { "DOWN" },
                srv.tools.len(),
                srv.argv.join(" "),
                srv.last_error.as_ref().map(|e| format!("\n   last error: {e}")).unwrap_or_default()
            ));
        }
        ToolResult::text(s)
    }

    fn restart(&mut self, name: &str) -> ToolResult {
        let targets: Vec<usize> = self
            .servers
            .iter()
            .enumerate()
            .filter(|(_, s)| name == "all" || s.name == name)
            .map(|(i, _)| i)
            .collect();
        if targets.is_empty() {
            return ToolResult::error(format!("no server named {name}"));
        }
        let mut out = Vec::new();
        for i in targets {
            let s = &mut self.servers[i];
            match s.start() {
                Ok(()) => out.push(format!("{}: restarted ({} tools)", s.name, s.tools.len())),
                Err(e) => {
                    s.last_error = Some(e.clone());
                    out.push(format!("{}: FAILED: {e}", s.name));
                }
            }
        }
        ToolResult::text(out.join("\n") + "\n(tool list may have changed — the client may need to reconnect)")
    }

    fn route(&mut self, full: &str, args: &Value) -> ToolResult {
        let Some((srv, tool)) = full.split_once(SEP) else {
            return ToolResult::error(format!("tool name must be <server>{SEP}<tool>: {full}"));
        };
        let Some(s) = self.servers.iter_mut().find(|s| s.name == srv) else {
            return ToolResult::error(format!("unknown server {srv}"));
        };
        if !s.alive() {
            if let Err(e) = s.start() {
                s.last_error = Some(e.clone());
                return ToolResult::error(format!("{srv} is down and failed to restart: {e}"));
            }
        }
        match s.request("tools/call", obj([("name", tool.into()), ("arguments", args.clone())])) {
            Ok(r) => ToolResult {
                content: r.get("content").and_then(|c| c.as_array()).cloned().unwrap_or_default(),
                is_error: r.bool_field("isError").unwrap_or(false),
            },
            Err(e) => {
                s.last_error = Some(e.clone());
                ToolResult::error(e)
            }
        }
    }
}

impl Handler for Gateway {
    fn tools(&mut self) -> Vec<ToolDef> {
        let mut v = vec![
            ToolDef {
                name: format!("mcpd{SEP}status"),
                description: "[mcpd] List MCP servers behind the gateway, their state and tool counts.".into(),
                input_schema: schema(&[], &[]),
            },
            ToolDef {
                name: format!("mcpd{SEP}restart"),
                description: "[mcpd] Restart one server by name, or 'all' (e.g. after rebuilding a server binary).".into(),
                input_schema: schema(&[("server", "string", "Server name or 'all'")], &["server"]),
            },
        ];
        for s in &self.servers {
            for t in &s.tools {
                v.push(ToolDef {
                    name: format!("{}{SEP}{}", s.name, t.name),
                    description: format!("[{}] {}", s.name, t.description),
                    input_schema: t.input_schema.clone(),
                });
            }
        }
        v
    }

    fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        let t0 = Instant::now();
        let r = match name {
            "mcpd__status" => self.status(),
            "mcpd__restart" => match args.str_field("server") {
                Some(s) => {
                    let s = s.to_string();
                    self.restart(&s)
                }
                None => ToolResult::error("missing 'server'"),
            },
            _ => self.route(name, args),
        };
        self.audit(name, args, &r, t0.elapsed().as_millis());
        r
    }
}

fn config_path() -> Option<String> {
    if let Ok(p) = std::env::var("MCPD_CONFIG") {
        return Some(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    [format!("{home}/.config/mcpd/servers.conf"), "/etc/mcpd/servers.conf".to_string()]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
}

fn load_config(path: &str) -> Vec<Server> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("mcpd: cannot read {path}: {e}");
        String::new()
    });
    text.lines()
        .map(|l| l.split('#').next().unwrap().trim())
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let (name, cmd) = l.split_once('=')?;
            let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
            let name = name.trim();
            if argv.is_empty() || name.contains(SEP) || name.is_empty() {
                eprintln!("mcpd: skipping bad config line: {l}");
                return None;
            }
            Some(Server::new(name.to_string(), argv))
        })
        .collect()
}

fn open_audit() -> Option<std::fs::File> {
    let dir = std::env::var("XDG_STATE_HOME")
        .unwrap_or_else(|_| format!("{}/.local/state", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())));
    let dir = format!("{dir}/mcpd");
    std::fs::create_dir_all(&dir).ok()?;
    OpenOptions::new().create(true).append(true).open(format!("{dir}/audit.jsonl")).ok()
}

fn main() {
    if std::env::args().any(|a| a == "--version") {
        println!("mcpd {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let Some(path) = config_path() else {
        eprintln!("mcpd: no servers.conf found ($MCPD_CONFIG, ~/.config/mcpd/servers.conf, /etc/mcpd/servers.conf)");
        std::process::exit(1);
    };
    let mut gw = Gateway { servers: load_config(&path), audit: open_audit() };
    for s in &mut gw.servers {
        if let Err(e) = s.start() {
            eprintln!("mcpd: {} failed to start: {e}", s.name);
            s.last_error = Some(e);
        }
    }
    eprintln!("mcpd: config {path}, {} servers", gw.servers.len());
    serve_stdio(
        "mcpd",
        env!("CARGO_PKG_VERSION"),
        Some("mcpOS gateway. Tools are named <server>__<tool>. This is a disposable test VM: root, shell, \
              desktop control and debugging are all allowed. Use mcpd__status to see what's available."),
        &mut gw,
    );
    for s in &mut gw.servers {
        s.stop();
    }
}
