//! mcp-a11y: read and drive any accessible app (GTK, Qt, Firefox/Chromium, LibreOffice…) through AT-SPI.
//! The AT-SPI work is done by an embedded Python helper (GObject Atspi bindings); this binary
//! handles MCP, timeouts (a hung app can't hang the agent) and enabling accessibility on the bus.

use mcp_core::json::{self, Value};
use mcp_core::proc::run;
use mcp_core::util::schema;
use mcp_core::{serve_stdio, ToolResult, Tools};
use std::time::Duration;

const HELPER: &str = include_str!("a11y.py");

fn helper(cmd: &str, args: &Value, timeout_s: u64) -> ToolResult {
    let mut req = match args {
        Value::Obj(_) => args.clone(),
        _ => Value::Obj(vec![]),
    };
    req.set("cmd", cmd.into());
    let python = std::env::var("MCP_A11Y_PYTHON").unwrap_or_else(|_| "python3".into());
    match run(&python, &["-I", "-", &req.to_json()], Some(HELPER.as_bytes()), Duration::from_secs(timeout_s)) {
        Err(e) => ToolResult::error(e),
        Ok(o) if o.timed_out => ToolResult::error(format!(
            "AT-SPI call timed out after {timeout_s}s — the target app is probably hung (a bug worth reporting). stderr: {}",
            o.stderr.trim()
        )),
        Ok(o) => {
            let line = o.stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
            match json::parse(line) {
                Ok(v) if v.bool_field("ok") == Some(true) => ToolResult::text(v.str_field("text").unwrap_or("").to_string()),
                Ok(v) => ToolResult::error(v.str_field("error").unwrap_or("unknown error").to_string()),
                Err(_) => ToolResult::error(format!("helper failed (exit {:?})\n{}\n{}", o.code, o.stdout, o.stderr)),
            }
        }
    }
}

/// Tell toolkits an assistive technology is present (org.a11y.Status.IsEnabled = true).
fn enable_bus() -> ToolResult {
    let r = run(
        "busctl",
        &["--user", "set-property", "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Status", "IsEnabled", "b", "true"],
        None,
        Duration::from_secs(10),
    );
    match r {
        Ok(o) if o.ok() => ToolResult::text(
            "AT-SPI enabled (org.a11y.Status.IsEnabled=true). Apps started *before* this may need a restart to expose their tree.",
        ),
        Ok(o) => ToolResult::error(o.render(4000)),
        Err(e) => ToolResult::error(e),
    }
}

fn main() {
    let _ = enable_bus(); // best effort at startup
    let app = ("app", "string", "App name (substring, case-insensitive) or PID");
    let path = ("path", "string", "Node path from tree/find, e.g. '0.2.5'");
    let mut tools = Tools::new()
        .tool("enable", "Turn on accessibility on the session bus (done automatically at startup).", schema(&[], &[]), |_| enable_bus())
        .tool("list_apps", "Applications currently exposing an accessibility tree (name, pid, windows, toolkit).",
            schema(&[], &[]), |a| helper("list_apps", a, 20))
        .tool("tree",
            "Accessibility tree of an app: one line per node with [path] role \"name\" {states} actions=… @x,y wxh (window coords) text=…. \
             Hidden subtrees are skipped. Use only_interactive=true for a flat list of things you can click/type into.",
            schema(&[
                app, ("path", "string", "Start from this node instead of the app root"),
                ("max_depth", "integer", "Default 12"), ("max_nodes", "integer", "Default 400"),
                ("only_interactive", "boolean", "Only nodes with actions / focusable / editable"),
                ("include_hidden", "boolean", "Include non-showing nodes"),
            ], &["app"]),
            |a| helper("tree", a, 45))
        .tool("find", "Search an app's tree. Filters combine: role (exact, e.g. 'push button'), name/text (substring), state (e.g. 'focused').",
            schema(&[
                app, ("role", "string", "Exact role name"), ("name", "string", "Substring of accessible name"),
                ("text", "string", "Substring of text content"), ("state", "string", "State nick, e.g. focused, checked"),
                ("limit", "integer", "Max results (default 20)"), ("include_hidden", "boolean", "Search non-showing nodes too"),
            ], &["app"]),
            |a| helper("find", a, 45))
        .tool("wait_for", "Wait until an element matching the filters appears (same filters as find). Good after clicks that open dialogs.",
            schema(&[
                app, ("role", "string", "Exact role name"), ("name", "string", "Substring of name"),
                ("text", "string", "Substring of text"), ("state", "string", "State nick"),
                ("timeout_secs", "number", "Default 10"),
            ], &["app"]),
            |a| {
                let t = a.get("timeout_secs").and_then(|v| v.as_f64()).unwrap_or(10.0).clamp(1.0, 120.0) as u64;
                helper("wait_for", a, t + 15)
            })
        .tool("action", "Perform an accessible action on a node (default: its first action, usually click/press/activate).",
            schema(&[app, path, ("action", "string", "Action name from tree output")], &["app", "path"]),
            |a| helper("action", a, 20))
        .tool("focus", "Give keyboard focus to a node (then use desktop__type_text / desktop__key).",
            schema(&[app, path], &["app", "path"]), |a| helper("focus", a, 20))
        .tool("get_text", "Full text content of a node (entries, documents, terminals that support it).",
            schema(&[app, path, ("limit", "integer", "Max chars (default 20000)")], &["app", "path"]),
            |a| helper("get_text", a, 30))
        .tool("set_text", "Replace the content of an editable text node.",
            schema(&[app, path, ("text", "string", "New content")], &["app", "path", "text"]),
            |a| helper("set_text", a, 20));

    serve_stdio(
        "mcp-a11y",
        env!("CARGO_PKG_VERSION"),
        Some("Reads UIs semantically via AT-SPI. Loop: list_apps -> tree(only_interactive) -> action/focus -> wait_for -> tree. \
              Prefer this over screenshots to find buttons, labels, disabled states and error messages."),
        &mut tools,
    );
}
