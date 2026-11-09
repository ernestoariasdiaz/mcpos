//! MCP server loop over stdio (newline-delimited JSON-RPC 2.0).
//! Implements: initialize, notifications/initialized, ping, tools/list, tools/call.

use crate::json::{self, obj, Value};
use std::io::{self, BufRead, Write};

pub const DEFAULT_PROTOCOL: &str = "2025-06-18";

#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl ToolDef {
    pub fn to_value(&self) -> Value {
        obj([
            ("name", self.name.as_str().into()),
            ("description", self.description.as_str().into()),
            ("inputSchema", self.input_schema.clone()),
        ])
    }
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(s: impl Into<String>) -> Self {
        ToolResult { content: vec![text_block(s)], is_error: false }
    }
    pub fn error(s: impl Into<String>) -> Self {
        ToolResult { content: vec![text_block(s)], is_error: true }
    }
    pub fn with_image(mut self, b64: String, mime: &str) -> Self {
        self.content.push(obj([
            ("type", "image".into()),
            ("data", b64.into()),
            ("mimeType", mime.into()),
        ]));
        self
    }
    pub fn to_value(&self) -> Value {
        obj([
            ("content", Value::Arr(self.content.clone())),
            ("isError", self.is_error.into()),
        ])
    }
}

fn text_block(s: impl Into<String>) -> Value {
    obj([("type", "text".into()), ("text", Value::Str(s.into()))])
}

/// Anything that can list and call tools.
pub trait Handler {
    fn tools(&mut self) -> Vec<ToolDef>;
    fn call(&mut self, name: &str, args: &Value) -> ToolResult;
}

type ToolFn = Box<dyn FnMut(&Value) -> ToolResult>;

/// Static tool table: the common case for leaf servers.
#[derive(Default)]
pub struct Tools {
    entries: Vec<(ToolDef, ToolFn)>,
}

impl Tools {
    pub fn new() -> Self { Self::default() }
    pub fn tool(
        mut self,
        name: &str,
        description: &str,
        input_schema: Value,
        f: impl FnMut(&Value) -> ToolResult + 'static,
    ) -> Self {
        self.entries.push((
            ToolDef { name: name.into(), description: description.into(), input_schema },
            Box::new(f),
        ));
        self
    }
}

impl Handler for Tools {
    fn tools(&mut self) -> Vec<ToolDef> {
        self.entries.iter().map(|(d, _)| d.clone()).collect()
    }
    fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        match self.entries.iter_mut().find(|(d, _)| d.name == name) {
            Some((_, f)) => f(args),
            None => ToolResult::error(format!("unknown tool: {name}")),
        }
    }
}

fn send(out: &mut impl Write, v: &Value) {
    let _ = writeln!(out, "{}", v.to_json());
    let _ = out.flush();
}

fn reply(id: &Value, result: Value) -> Value {
    obj([("jsonrpc", "2.0".into()), ("id", id.clone()), ("result", result)])
}

fn reply_err(id: &Value, code: i64, msg: &str) -> Value {
    obj([
        ("jsonrpc", "2.0".into()),
        ("id", id.clone()),
        ("error", obj([("code", code.into()), ("message", msg.into())])),
    ])
}

/// Run until stdin closes.
pub fn serve_stdio(name: &str, version: &str, instructions: Option<&str>, h: &mut dyn Handler) {
    let stdin = io::stdin();
    let stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let mut out = stdout.lock();
        let msg = match json::parse(&line) {
            Ok(m) => m,
            Err(e) => {
                send(&mut out, &reply_err(&Value::Null, -32700, &format!("parse error: {e}")));
                continue;
            }
        };
        let Some(method) = msg.str_field("method") else { continue }; // a response; ignore
        let Some(id) = msg.get("id").cloned() else { continue }; // notification; ignore
        let params = msg.get("params").cloned().unwrap_or(Value::Obj(vec![]));

        let resp = match method {
            "initialize" => {
                let proto = params.str_field("protocolVersion").unwrap_or(DEFAULT_PROTOCOL);
                let mut r = obj([
                    ("protocolVersion", proto.into()),
                    ("capabilities", obj([("tools", obj([("listChanged", false.into())]))])),
                    ("serverInfo", obj([("name", name.into()), ("version", version.into())])),
                ]);
                if let Some(i) = instructions {
                    r.set("instructions", i.into());
                }
                reply(&id, r)
            }
            "ping" => reply(&id, Value::Obj(vec![])),
            "tools/list" => {
                let tools: Vec<Value> = h.tools().iter().map(|t| t.to_value()).collect();
                reply(&id, obj([("tools", Value::Arr(tools))]))
            }
            "tools/call" => match params.str_field("name") {
                None => reply_err(&id, -32602, "missing tool name"),
                Some(tool) => {
                    let args = params.get("arguments").cloned().unwrap_or(Value::Obj(vec![]));
                    let tool = tool.to_string();
                    drop(out); // don't hold stdout while a long tool runs
                    let r = h.call(&tool, &args);
                    let mut out = stdout.lock();
                    send(&mut out, &reply(&id, r.to_value()));
                    continue;
                }
            },
            "resources/list" => reply(&id, obj([("resources", Value::Arr(vec![]))])),
            "prompts/list" => reply(&id, obj([("prompts", Value::Arr(vec![]))])),
            _ => reply_err(&id, -32601, &format!("method not found: {method}")),
        };
        send(&mut out, &resp);
    }
}
