//! mcp-desktop: drive a Sway (wlroots) desktop — windows, input, screenshots, clipboard, notifications.

use mcp_core::json::{self, Value};
use mcp_core::proc::{run, Output};
use mcp_core::util::{base64, schema};
use mcp_core::{serve_stdio, ToolResult, Tools};
use std::time::Duration;

const T: Duration = Duration::from_secs(20);

/// Make sure SWAYSOCK / WAYLAND_DISPLAY / XDG_RUNTIME_DIR are set even if we were
/// launched outside the session (e.g. from a TTY or ssh).
fn fix_env() {
    let uid = run("id", &["-u"], None, T).map(|o| o.stdout.trim().to_string()).unwrap_or_default();
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{uid}"));
    std::env::set_var("XDG_RUNTIME_DIR", &rt);
    let entries: Vec<String> = std::fs::read_dir(&rt)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    if std::env::var("SWAYSOCK").is_err() {
        if let Some(s) = entries.iter().find(|n| n.starts_with("sway-ipc.") && n.ends_with(".sock")) {
            std::env::set_var("SWAYSOCK", format!("{rt}/{s}"));
        }
    }
    if std::env::var("WAYLAND_DISPLAY").is_err() {
        if let Some(w) = entries.iter().find(|n| n.starts_with("wayland-") && !n.ends_with(".lock")) {
            std::env::set_var("WAYLAND_DISPLAY", w);
        }
    }
}

fn out(r: Result<Output, String>) -> ToolResult {
    match r {
        Ok(o) if o.ok() => ToolResult::text(if o.stdout.trim().is_empty() { "ok".into() } else { o.stdout }),
        Ok(o) => ToolResult::error(o.render(20_000)),
        Err(e) => ToolResult::error(e),
    }
}

fn swaymsg(args: &[&str]) -> Result<Output, String> {
    run("swaymsg", args, None, T)
}

fn sway_cmd(cmd: &str) -> ToolResult {
    match swaymsg(&["--", cmd]) {
        Ok(o) => {
            // swaymsg prints [{"success":false,"error":"..."}] on failure
            if o.stdout.contains("\"success\": false") || o.stdout.contains("\"success\":false") || !o.ok() {
                ToolResult::error(o.render(10_000))
            } else {
                ToolResult::text(format!("ok: {cmd}"))
            }
        }
        Err(e) => ToolResult::error(e),
    }
}

/// Compact, LLM-friendly view of the sway tree.
fn summarize(node: &Value, depth: usize, out: &mut String) {
    let ty = node.str_field("type").unwrap_or("?");
    let id = node.i64_field("id").unwrap_or(0);
    let name = node.str_field("name").unwrap_or("");
    let focused = node.bool_field("focused").unwrap_or(false);
    let pad = "  ".repeat(depth);
    let rect = node.get("rect").map(|r| {
        format!(
            "{}x{}+{}+{}",
            r.i64_field("width").unwrap_or(0),
            r.i64_field("height").unwrap_or(0),
            r.i64_field("x").unwrap_or(0),
            r.i64_field("y").unwrap_or(0)
        )
    }).unwrap_or_default();
    let is_window = node.get("pid").map(|p| !p.is_null()).unwrap_or(false);
    match ty {
        "root" => {}
        "output" if name == "__i3" => return, // scratchpad holder
        "output" => out.push_str(&format!("{pad}output {name} {rect}\n")),
        "workspace" => out.push_str(&format!("{pad}workspace {name}{}\n", if focused { " *focused*" } else { "" })),
        _ if is_window => {
            let app = node.str_field("app_id").map(|s| s.to_string())
                .or_else(|| node.get("window_properties").and_then(|w| w.str_field("class")).map(|c| format!("{c} (xwayland)")))
                .unwrap_or_else(|| "?".into());
            out.push_str(&format!(
                "{pad}window con_id={id} pid={} app={app} title={:?} {rect}{}{}\n",
                node.i64_field("pid").unwrap_or(0),
                name,
                if focused { " *focused*" } else { "" },
                if node.bool_field("urgent").unwrap_or(false) { " urgent" } else { "" },
            ));
        }
        _ => {
            if let Some(l) = node.str_field("layout") {
                out.push_str(&format!("{pad}container con_id={id} layout={l}\n"));
            }
        }
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(kids) = node.get(key).and_then(|k| k.as_array()) {
            for k in kids {
                summarize(k, depth + 1, out);
            }
        }
    }
}

fn get_tree(a: &Value) -> ToolResult {
    let raw = a.bool_field("raw").unwrap_or(false);
    match swaymsg(&["-r", "-t", "get_tree"]) {
        Ok(o) if o.ok() => {
            if raw {
                return ToolResult::text(mcp_core::util::truncate(&o.stdout, 120_000));
            }
            match json::parse(&o.stdout) {
                Ok(v) => {
                    let mut s = String::new();
                    summarize(&v, 0, &mut s);
                    ToolResult::text(s)
                }
                Err(e) => ToolResult::error(format!("parse tree: {e}")),
            }
        }
        r => out(r),
    }
}

fn screenshot(a: &Value) -> ToolResult {
    let path = a.str_field("save_to").map(|s| s.to_string()).unwrap_or_else(|| {
        format!("/tmp/mcp-shot-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
    });
    let mut args: Vec<String> = Vec::new();
    if let Some(r) = a.str_field("region") { args.push("-g".into()); args.push(r.into()); }
    if let Some(o) = a.str_field("output") { args.push("-o".into()); args.push(o.into()); }
    // scale down by default: big PNGs burn context
    let scale = a.get("scale").and_then(|s| s.as_f64()).unwrap_or(0.5);
    args.push("-s".into());
    args.push(format!("{scale}"));
    args.push(path.clone());
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match run("grim", &argv, None, T) {
        Ok(o) if o.ok() => match std::fs::read(&path) {
            Ok(bytes) => ToolResult::text(format!("saved {path} ({} bytes, scale {scale})", bytes.len()))
                .with_image(base64(&bytes), "image/png"),
            Err(e) => ToolResult::error(format!("read {path}: {e}")),
        },
        r => out(r),
    }
}

/// "ctrl+shift+t" -> wtype -M ctrl -M shift -k t -m shift -m ctrl
fn key(a: &Value) -> ToolResult {
    let Some(combo) = a.str_field("keys") else { return ToolResult::error("missing 'keys'") };
    let mut results = Vec::new();
    for chord in combo.split_whitespace() {
        let parts: Vec<&str> = chord.split('+').collect();
        let (k, mods) = parts.split_last().unwrap();
        let mods: Vec<&str> = mods.iter().map(|m| match m.to_lowercase().as_str() {
            "super" | "win" | "meta" | "mod4" => "logo",
            "control" => "ctrl",
            _ => m,
        }).collect();
        let mut args: Vec<String> = Vec::new();
        for m in &mods { args.push("-M".into()); args.push(m.to_lowercase()); }
        args.push("-k".into());
        args.push(k.to_string());
        for m in mods.iter().rev() { args.push("-m".into()); args.push(m.to_lowercase()); }
        let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match run("wtype", &argv, None, T) {
            Ok(o) if o.ok() => results.push(format!("sent {chord}")),
            r => return out(r),
        }
    }
    ToolResult::text(results.join("\n"))
}

fn pointer(a: &Value) -> ToolResult {
    let x = a.i64_field("x");
    let y = a.i64_field("y");
    let mut done = Vec::new();
    if let (Some(x), Some(y)) = (x, y) {
        let r = sway_cmd(&format!("seat seat0 cursor set {x} {y}"));
        if r.is_error { return r; }
        done.push(format!("moved to {x},{y}"));
    }
    let button = match a.str_field("button").unwrap_or("none") {
        "left" => Some("button1"),
        "middle" => Some("button2"),
        "right" => Some("button3"),
        "scroll_up" => Some("button4"),
        "scroll_down" => Some("button5"),
        _ => None,
    };
    if let Some(b) = button {
        let clicks = a.i64_field("clicks").unwrap_or(1).clamp(1, 3);
        for _ in 0..clicks {
            for act in ["press", "release"] {
                let r = sway_cmd(&format!("seat seat0 cursor {act} {b}"));
                if r.is_error { return r; }
            }
        }
        done.push(format!("{b} x{clicks}"));
    }
    if done.is_empty() { ToolResult::error("give x,y and/or button") } else { ToolResult::text(done.join(", ")) }
}

fn main() {
    fix_env();
    let mut tools = Tools::new()
        .tool("get_tree", "Outputs, workspaces and windows (con_id, pid, app_id, title, geometry, focus). raw=true returns full sway JSON.",
            schema(&[("raw", "boolean", "Return raw sway JSON")], &[]), get_tree)
        .tool("get_workspaces", "List workspaces.", schema(&[], &[]), |_| out(swaymsg(&["-t", "get_workspaces"])))
        .tool("get_outputs", "List monitors with modes and scale.", schema(&[], &[]), |_| out(swaymsg(&["-t", "get_outputs"])))
        .tool("sway_command",
            "Run any sway command. Examples: '[con_id=12] focus', 'workspace 3', 'move container to workspace 2', \
             'floating toggle', 'fullscreen', 'kill', 'output * scale 1.25', 'reload'.",
            schema(&[("command", "string", "sway command")], &["command"]),
            |a| match a.str_field("command") { Some(c) => sway_cmd(c), None => ToolResult::error("missing 'command'") })
        .tool("launch", "Launch an app inside the session (swaymsg exec), e.g. 'foot', 'firefox'.",
            schema(&[("command", "string", "Command line")], &["command"]),
            |a| match a.str_field("command") { Some(c) => sway_cmd(&format!("exec {c}")), None => ToolResult::error("missing 'command'") })
        .tool("screenshot", "Capture the screen (or a region 'X,Y WxH', or one output) and return it as an image.",
            schema(&[
                ("region", "string", "Geometry 'X,Y WxH' (e.g. from get_tree rect)"),
                ("output", "string", "Output name, e.g. Virtual-1"),
                ("scale", "number", "Image scale (default 0.5)"),
                ("save_to", "string", "PNG path (default /tmp/mcp-shot-*.png)"),
            ], &[]), screenshot)
        .tool("type_text", "Type text into the focused window.",
            schema(&[("text", "string", "Text to type")], &["text"]),
            |a| match a.str_field("text") { Some(t) => out(run("wtype", &["--", t], None, T)), None => ToolResult::error("missing 'text'") })
        .tool("key", "Press key chords, space separated. Key names are xkb keysyms: 'Return', 'Escape', 'ctrl+l', 'super+Return', 'alt+F4', 'ctrl+shift+t'.",
            schema(&[("keys", "string", "Chord(s)")], &["keys"]), key)
        .tool("pointer", "Move the pointer to absolute layout coords and/or click (left|middle|right|scroll_up|scroll_down).",
            schema(&[
                ("x", "integer", "X in layout coordinates"),
                ("y", "integer", "Y in layout coordinates"),
                ("button", "string", "left|middle|right|scroll_up|scroll_down"),
                ("clicks", "integer", "1-3 (default 1)"),
            ], &[]), pointer)
        .tool("clipboard_get", "Read the clipboard as text.", schema(&[], &[]),
            |_| out(run("wl-paste", &["--no-newline"], None, T)))
        .tool("clipboard_set", "Set the clipboard.", schema(&[("text", "string", "Text")], &["text"]),
            |a| match a.str_field("text") {
                // wl-copy forks a server for the selection; detach its stdio so we don't wait on it
                Some(t) => out(run("sh", &["-c", "wl-copy -- \"$1\" >/dev/null 2>&1 </dev/null", "sh", t], None, T)),
                None => ToolResult::error("missing 'text'"),
            })
        .tool("notify", "Show a desktop notification.", schema(&[("summary", "string", "Title"), ("body", "string", "Body")], &["summary"]),
            |a| out(run("notify-send", &["--app-name=mcpOS", a.str_field("summary").unwrap_or("mcpOS"), a.str_field("body").unwrap_or("")], None, T)));

    serve_stdio(
        "mcp-desktop",
        env!("CARGO_PKG_VERSION"),
        Some("Controls the Sway desktop. Typical loop: get_tree -> launch/focus -> screenshot -> key/type_text/pointer -> screenshot."),
        &mut tools,
    );
}
