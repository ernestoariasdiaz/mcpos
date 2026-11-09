//! KDE Plasma (Wayland) backend:
//!  - windows / virtual desktops: KWin scripting via the embedded kwin.py bridge
//!  - screenshots: spectacle (+ imagemagick for crop/scale)
//!  - keyboard / pointer: ydotool (uinput; KWin doesn't allow wtype)
//!  - clipboard: wl-clipboard, klipper as fallback

use mcp_core::json::{self, Value};
use mcp_core::proc::{run, Output};
use mcp_core::util::{base64, schema, truncate};
use mcp_core::{ToolResult, Tools};
use std::time::Duration;

const T: Duration = Duration::from_secs(20);
const SHOT_T: Duration = Duration::from_secs(45);
const KWIN_PY: &str = include_str!("kwin.py");

fn out(r: Result<Output, String>) -> ToolResult {
    match r {
        Ok(o) if o.ok() => ToolResult::text(if o.stdout.trim().is_empty() { "ok".into() } else { o.stdout }),
        Ok(o) => ToolResult::error(o.render(20_000)),
        Err(e) => ToolResult::error(e),
    }
}

/// Run an op in the KWin bridge and return its parsed JSON.
fn kwin(op: &str, args: &Value) -> Result<Value, String> {
    let mut req = match args { Value::Obj(_) => args.clone(), _ => Value::Obj(vec![]) };
    req.set("op", op.into());
    let python = std::env::var("MCP_DESKTOP_PYTHON").unwrap_or_else(|_| "python3".into());
    let o = run(&python, &["-I", "-", &req.to_json()], Some(KWIN_PY.as_bytes()), T)?;
    if o.timed_out {
        return Err("KWin bridge timed out (KWin hung?)".into());
    }
    let line = o.stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
    let v = json::parse(line).map_err(|_| format!("bridge failed: {}\n{}", o.stdout, o.stderr))?;
    if let Some(e) = v.str_field("error") {
        return Err(e.to_string());
    }
    Ok(v)
}

fn fmt_window(w: &Value) -> String {
    format!(
        "window id={} pid={} app={} title={:?} {}x{}+{}+{}{}{}{}",
        w.str_field("id").unwrap_or("?"),
        w.i64_field("pid").unwrap_or(0),
        w.str_field("app").unwrap_or("?"),
        w.str_field("caption").unwrap_or(""),
        w.i64_field("w").unwrap_or(0),
        w.i64_field("h").unwrap_or(0),
        w.i64_field("x").unwrap_or(0),
        w.i64_field("y").unwrap_or(0),
        if w.bool_field("active").unwrap_or(false) { " *active*" } else { "" },
        if w.bool_field("minimized").unwrap_or(false) { " minimized" } else { "" },
        if w.bool_field("fullscreen").unwrap_or(false) { " fullscreen" } else { "" },
    )
}

fn get_tree(a: &Value) -> ToolResult {
    match kwin("list", &Value::Obj(vec![])) {
        Err(e) => ToolResult::error(e),
        Ok(v) if a.bool_field("raw").unwrap_or(false) => ToolResult::text(v.to_json()),
        Ok(v) => {
            let mut s = String::new();
            for sc in v.get("screens").and_then(|x| x.as_array()).into_iter().flatten() {
                s.push_str(&format!(
                    "screen {} {}x{}+{}+{}\n",
                    sc.str_field("name").unwrap_or("?"),
                    sc.i64_field("w").unwrap_or(0), sc.i64_field("h").unwrap_or(0),
                    sc.i64_field("x").unwrap_or(0), sc.i64_field("y").unwrap_or(0)
                ));
            }
            let cur = v.str_field("current").unwrap_or("");
            let desks: Vec<String> = v.get("desktops").and_then(|x| x.as_array()).into_iter().flatten()
                .map(|d| {
                    let n = d.str_field("name").unwrap_or("?");
                    if n == cur { format!("{n} *current*") } else { n.to_string() }
                })
                .collect();
            s.push_str(&format!("desktops: {}\n", desks.join(", ")));
            for w in v.get("windows").and_then(|x| x.as_array()).into_iter().flatten() {
                let d = w.get("desktops").and_then(|x| x.as_array()).map(|a| {
                    a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")
                }).unwrap_or_default();
                s.push_str(&format!("  {} [{}]\n", fmt_window(w), if d.is_empty() { "all".into() } else { d }));
            }
            ToolResult::text(s)
        }
    }
}

fn window(a: &Value) -> ToolResult {
    if a.str_field("id").is_none() || a.str_field("action").is_none() {
        return ToolResult::error("need 'id' (window id, pid, or title/app substring) and 'action'");
    }
    match kwin("window", a) {
        Ok(v) => ToolResult::text(format!(
            "{}: {}",
            v.str_field("action").unwrap_or("ok"),
            v.get("window").map(fmt_window).unwrap_or_default()
        )),
        Err(e) => ToolResult::error(e),
    }
}

fn capture(mode: &str, raw: &str) -> Result<(), String> {
    // Spectacle is single-instance: if one is already open (e.g. the user is selecting a region),
    // our `spectacle -b` is forwarded to it and waits. Never kill it — it may be the user's.
    if run("pgrep", &["-x", "spectacle"], None, T).map(|o| o.ok()).unwrap_or(false) {
        return Err("Spectacle is already open (probably by the user, e.g. selecting a region). \
                    Ask them to finish or close it, then retry."
            .into());
    }
    let _ = std::fs::remove_file(raw);
    let o = run("spectacle", &["-b", "-n", mode, "-o", raw], None, SHOT_T)?;
    for _ in 0..50 {
        if std::fs::metadata(raw).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(if o.timed_out {
        format!("spectacle timed out after {}s", SHOT_T.as_secs())
    } else {
        format!("spectacle produced no file: {}", o.render(2000))
    })
}

fn screenshot(a: &Value) -> ToolResult {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let raw = format!("/tmp/mcp-shot-{ts}-raw.png");
    let path = a.str_field("save_to").map(|s| s.to_string()).unwrap_or_else(|| format!("/tmp/mcp-shot-{ts}.png"));

    // window=<id|title|app>: keep it above everything during the capture, then crop to its frame,
    // so it doesn't matter which window has focus (e.g. the agent's own terminal)
    let mut crop: Option<String> = a.str_field("region").map(String::from);
    let mut pinned: Option<String> = None;
    if let Some(wid) = a.str_field("window") {
        let mut req = Value::Obj(vec![]);
        req.set("id", wid.into());
        req.set("action", "above_on".into());
        match kwin("window", &req) {
            Ok(v) => {
                let w = v.get("window").cloned().unwrap_or(Value::Null);
                crop = Some(format!(
                    "{},{} {}x{}",
                    w.i64_field("x").unwrap_or(0).max(0), w.i64_field("y").unwrap_or(0).max(0),
                    w.i64_field("w").unwrap_or(0), w.i64_field("h").unwrap_or(0)
                ));
                pinned = Some(wid.to_string());
                std::thread::sleep(Duration::from_millis(350)); // let KWin repaint
            }
            Err(e) => return ToolResult::error(format!("window '{wid}': {e}")),
        }
    }
    let mode = if a.bool_field("active_window").unwrap_or(false) && pinned.is_none() { "-a" } else { "-f" };
    let shot = capture(mode, &raw);
    if let Some(wid) = &pinned {
        let mut req = Value::Obj(vec![]);
        req.set("id", wid.as_str().into());
        req.set("action", "above_off".into());
        let _ = kwin("window", &req);
    }
    if let Err(e) = shot {
        return ToolResult::error(e);
    }

    let scale = a.get("scale").and_then(|s| s.as_f64()).unwrap_or(0.5);
    let mut args: Vec<String> = vec![raw.clone()];
    if let Some(r) = crop {
        // "X,Y WxH" -> WxH+X+Y
        let geo = r.replace(' ', ",");
        let p: Vec<&str> = geo.split([',', 'x']).filter(|s| !s.is_empty()).collect();
        if p.len() == 4 {
            args.push("-crop".into());
            args.push(format!("{}x{}+{}+{}", p[2], p[3], p[0], p[1]));
            args.push("+repage".into());
        } else {
            return ToolResult::error("region must be 'X,Y WxH'");
        }
    }
    if (scale - 1.0).abs() > 1e-3 {
        args.push("-resize".into());
        args.push(format!("{}%", (scale * 100.0).round()));
    }
    args.push(path.clone());
    let argv: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let final_path = match run("magick", &argv, None, T) {
        Ok(o) if o.ok() => { let _ = std::fs::remove_file(&raw); path }
        _ => raw, // no imagemagick: return the full-size capture
    };
    match std::fs::read(&final_path) {
        Ok(b) => ToolResult::text(format!("saved {final_path} ({} bytes, scale {scale})", b.len())).with_image(base64(&b), "image/png"),
        Err(e) => ToolResult::error(format!("read {final_path}: {e}")),
    }
}

/// Linux input event codes for ydotool.
fn keycode(name: &str) -> Option<u32> {
    let n = name.to_lowercase();
    let letters = "qwertyuiop"; // 16..25
    if n.len() == 1 {
        let c = n.chars().next().unwrap();
        if let Some(i) = letters.find(c) { return Some(16 + i as u32); }
        if let Some(i) = "asdfghjkl".find(c) { return Some(30 + i as u32); }
        if let Some(i) = "zxcvbnm".find(c) { return Some(44 + i as u32); }
        if let Some(i) = "1234567890".find(c) { return Some(2 + i as u32); }
        return match c {
            '-' => Some(12), '=' => Some(13), '[' => Some(26), ']' => Some(27), ';' => Some(39),
            '\'' => Some(40), '`' => Some(41), '\\' => Some(43), ',' => Some(51), '.' => Some(52),
            '/' => Some(53), ' ' => Some(57), _ => None,
        };
    }
    if let Some(f) = n.strip_prefix('f').and_then(|x| x.parse::<u32>().ok()) {
        return match f { 1..=10 => Some(58 + f), 11 => Some(87), 12 => Some(88), _ => None };
    }
    Some(match n.as_str() {
        "escape" | "esc" => 1, "minus" => 12, "equal" => 13, "backspace" => 14, "tab" => 15,
        "bracketleft" => 26, "bracketright" => 27, "return" | "enter" => 28, "semicolon" => 39,
        "apostrophe" => 40, "grave" => 41, "backslash" => 43, "comma" => 51, "period" => 52,
        "slash" => 53, "space" => 57, "caps_lock" => 58, "print" => 99, "home" => 102, "up" => 103,
        "page_up" | "prior" => 104, "left" => 105, "right" => 106, "end" => 107, "down" => 108,
        "page_down" | "next" => 109, "insert" => 110, "delete" => 111, "menu" => 127,
        "ctrl" | "control" | "control_l" => 29, "shift" | "shift_l" => 42, "alt" | "alt_l" => 56,
        "super" | "logo" | "win" | "meta" | "mod4" | "super_l" => 125, "altgr" | "alt_r" => 100,
        "control_r" => 97, "shift_r" => 54,
        _ => return None,
    })
}

fn ydotool(args: &[&str]) -> Result<Output, String> {
    let r = run("ydotool", args, None, T)?;
    if !r.ok() && (r.stderr.contains("socket") || r.stderr.contains("ydotoold")) {
        return Err(format!(
            "ydotool can't reach ydotoold: {}\nfix: systemctl --user enable --now mcpos-ydotoold",
            r.stderr.trim()
        ));
    }
    Ok(r)
}

fn key(a: &Value) -> ToolResult {
    let Some(combo) = a.str_field("keys") else { return ToolResult::error("missing 'keys'") };
    let mut done = Vec::new();
    for chord in combo.split_whitespace() {
        let parts: Vec<&str> = chord.split('+').collect();
        let mut codes = Vec::new();
        for p in &parts {
            match keycode(p) {
                Some(c) => codes.push(c),
                None => return ToolResult::error(format!("unknown key '{p}' in '{chord}'")),
            }
        }
        let mut ev: Vec<String> = codes.iter().map(|c| format!("{c}:1")).collect();
        ev.extend(codes.iter().rev().map(|c| format!("{c}:0")));
        let mut argv = vec!["key"];
        argv.extend(ev.iter().map(|s| s.as_str()));
        match ydotool(&argv) {
            Ok(o) if o.ok() => done.push(format!("sent {chord}")),
            r => return out(r),
        }
    }
    ToolResult::text(done.join("\n"))
}

fn type_text(a: &Value) -> ToolResult {
    let Some(text) = a.str_field("text") else { return ToolResult::error("missing 'text'") };
    if text.is_ascii() {
        return match ydotool(&["type", "--key-delay", "4", "--", text]) {
            Ok(o) if o.ok() => ToolResult::text(format!("typed {} chars", text.chars().count())),
            r => out(r),
        };
    }
    // ydotool only types US-ASCII: paste non-ASCII text through the clipboard instead
    let paste = a.str_field("paste_keys").unwrap_or("ctrl+v");
    let copy = run("sh", &["-c", "wl-copy -- \"$1\" >/dev/null 2>&1 </dev/null", "sh", text], None, T);
    if !matches!(&copy, Ok(o) if o.ok()) {
        return ToolResult::error("non-ASCII text needs wl-copy for the clipboard fallback");
    }
    std::thread::sleep(Duration::from_millis(150));
    let mut k = Value::Obj(vec![]);
    k.set("keys", paste.into());
    let r = key(&k);
    if r.is_error { return r; }
    ToolResult::text(format!(
        "non-ASCII text pasted via clipboard + {paste} (in Konsole use paste_keys='ctrl+shift+v')"
    ))
}

/// Quoted strings from busctl output, e.g. `as 2 "event0" "event1"` -> ["event0","event1"].
fn busctl_strings(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut esc = false;
    for ch in s.chars() {
        match (&mut cur, ch) {
            (None, '"') => cur = Some(String::new()),
            (Some(_), '\\') if !esc => esc = true,
            (Some(c), '"') if !esc => { out.push(std::mem::take(c)); cur = None; }
            (Some(c), x) => { c.push(x); esc = false; }
            _ => {}
        }
    }
    out
}

/// Make KWin treat ydotool's virtual pointer linearly (flat profile, neutral speed),
/// so absolute moves land where they should. Done once per process, best effort.
fn ensure_flat_pointer() -> String {
    static DONE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DONE.get_or_init(|| {
        let bus = |args: &[&str]| run("busctl", args, None, T).ok().filter(|o| o.ok()).map(|o| o.stdout);
        let Some(list) = bus(&["--user", "get-property", "org.kde.KWin", "/org/kde/KWin/InputDevice",
                               "org.kde.KWin.InputDeviceManager", "devicesSysNames"]) else {
            return "could not list KWin input devices".into();
        };
        let mut done = Vec::new();
        for dev in busctl_strings(&list) {
            let path = format!("/org/kde/KWin/InputDevice/{dev}");
            let name = bus(&["--user", "get-property", "org.kde.KWin", &path, "org.kde.KWin.InputDevice", "name"])
                .map(|n| busctl_strings(&n).join("")).unwrap_or_default();
            if !name.to_lowercase().contains("ydotool") { continue; }
            let _ = bus(&["--user", "set-property", "org.kde.KWin", &path, "org.kde.KWin.InputDevice",
                          "pointerAccelerationProfileFlat", "b", "true"]);
            let _ = bus(&["--user", "set-property", "org.kde.KWin", &path, "org.kde.KWin.InputDevice",
                          "pointerAcceleration", "d", "0"]);
            done.push(format!("{dev} ({name})"));
        }
        if done.is_empty() { "ydotool device not found in KWin yet".into() } else { format!("flat accel on {}", done.join(", ")) }
    }).clone()
}

fn cursor_pos() -> Option<(i64, i64)> {
    let v = kwin("cursor", &Value::Obj(vec![])).ok()?;
    Some((v.i64_field("x")?, v.i64_field("y")?))
}

/// Move to (x,y) and verify against KWin's real cursor, correcting with a measured gain per axis
/// (handles residual pointer acceleration). Returns Ok(note) when within 2 px.
fn move_verified(x: i64, y: i64) -> Result<String, String> {
    let accel = ensure_flat_pointer();
    let (xs, ys) = (x.to_string(), y.to_string());
    match ydotool(&["mousemove", "--absolute", "-x", &xs, "-y", &ys]) {
        Ok(o) if o.ok() => {}
        Ok(o) => return Err(o.render(2000)),
        Err(e) => return Err(e),
    }
    let (mut gx, mut gy) = (1.0f64, 1.0f64);
    let mut last: Option<(i64, i64, i64, i64)> = None; // (pos before, commanded) of the previous correction
    for i in 0..8 {
        std::thread::sleep(Duration::from_millis(60));
        let Some((cx, cy)) = cursor_pos() else {
            return Ok(format!("moved (unverified: KWin cursor not readable); {accel}"));
        };
        if let Some((px, py, cmx, cmy)) = last {
            // learn the effective gain from what the last correction actually did
            if cmx.abs() >= 3 { let g = (cx - px) as f64 / cmx as f64; if g > 0.2 && g < 5.0 { gx = g; } }
            if cmy.abs() >= 3 { let g = (cy - py) as f64 / cmy as f64; if g > 0.2 && g < 5.0 { gy = g; } }
        }
        let (ex, ey) = (x - cx, y - cy);
        if ex.abs() <= 2 && ey.abs() <= 2 {
            return Ok(format!("verified at {cx},{cy} after {i} correction(s); {accel}"));
        }
        let (cmx, cmy) = ((ex as f64 / gx).round() as i64, (ey as f64 / gy).round() as i64);
        let (a1, a2) = (cmx.to_string(), cmy.to_string());
        if !matches!(ydotool(&["mousemove", "-x", &a1, "-y", &a2]), Ok(ref o) if o.ok()) { break; }
        last = Some((cx, cy, cmx, cmy));
    }
    let (cx, cy) = cursor_pos().unwrap_or((-1, -1));
    Err(format!("pointer did not reach {x},{y} (ended at {cx},{cy}); {accel}. \
                 Prefer a11y__action for clicking UI elements."))
}

fn pointer(a: &Value) -> ToolResult {
    let mut done = Vec::new();
    if let (Some(x), Some(y)) = (a.i64_field("x"), a.i64_field("y")) {
        match move_verified(x, y) {
            Ok(note) => done.push(format!("moved to {x},{y} — {note}")),
            // never click somewhere we didn't mean to
            Err(e) => return ToolResult::error(format!("{e}\nno click was sent")),
        }
    }
    let clicks = a.i64_field("clicks").unwrap_or(1).clamp(1, 3).to_string();
    let btn = a.str_field("button").unwrap_or("none");
    let r = match btn {
        "left" => Some(ydotool(&["click", "--repeat", &clicks, "0xC0"])),
        "right" => Some(ydotool(&["click", "--repeat", &clicks, "0xC1"])),
        "middle" => Some(ydotool(&["click", "--repeat", &clicks, "0xC2"])),
        "scroll_up" => Some(ydotool(&["mousemove", "--wheel", "-x", "0", "-y", "1"])),
        "scroll_down" => Some(ydotool(&["mousemove", "--wheel", "-x", "0", "-y", "-1"])),
        _ => None,
    };
    if let Some(r) = r {
        match r {
            Ok(o) if o.ok() => done.push(format!("{btn} x{clicks}")),
            r => return out(r),
        }
    }
    if done.is_empty() { ToolResult::error("give x,y and/or button") } else { ToolResult::text(done.join(", ")) }
}

fn clipboard_get(_: &Value) -> ToolResult {
    if let Ok(o) = run("wl-paste", &["--no-newline"], None, T) {
        if o.ok() { return ToolResult::text(o.stdout); }
    }
    match run("busctl", &["--user", "call", "org.kde.klipper", "/klipper", "org.kde.klipper.klipper", "getClipboardContents"], None, T) {
        Ok(o) if o.ok() => {
            // busctl prints: s "text"
            let t = o.stdout.trim().strip_prefix("s ").unwrap_or(o.stdout.trim());
            ToolResult::text(json::parse(t).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| t.to_string()))
        }
        r => out(r),
    }
}

fn launch(a: &Value) -> ToolResult {
    let Some(c) = a.str_field("command") else { return ToolResult::error("missing 'command'") };
    // detach from us so the app outlives this server
    match run("sh", &["-c", "setsid -f sh -c \"$1\" >/dev/null 2>&1 </dev/null", "sh", c], None, T) {
        Ok(o) if o.ok() => ToolResult::text(format!("launched: {c} (use get_tree or a11y__wait_for to see it)")),
        r => out(r),
    }
}

pub fn tools() -> Tools {
    Tools::new()
        .tool("get_tree", "Screens, virtual desktops and windows (id, pid, app, title, geometry, active/minimized). raw=true for JSON.",
            schema(&[("raw", "boolean", "Return raw JSON")], &[]), get_tree)
        .tool("window",
            "Manage a window. id = window id from get_tree, a PID, or a title/app substring. \
             action: activate | close | minimize | unminimize | maximize | unmaximize | fullscreen | keep_above | move (x,y,w,h) | to_desktop (desktop).",
            schema(&[
                ("id", "string", "Window id, pid or title/app substring"),
                ("action", "string", "See description"),
                ("x", "integer", "move: x"), ("y", "integer", "move: y"),
                ("w", "integer", "move: width"), ("h", "integer", "move: height"),
                ("desktop", "string", "to_desktop: name or number"),
            ], &["id", "action"]), window)
        .tool("switch_desktop", "Switch to a virtual desktop by name or number.",
            schema(&[("desktop", "string", "Name or number")], &["desktop"]),
            |a| match kwin("desktop", a) {
                Ok(v) => ToolResult::text(format!("current desktop: {}", v.str_field("current").unwrap_or("?"))),
                Err(e) => ToolResult::error(e),
            })
        .tool("get_outputs", "Monitors, modes and scale (kscreen-doctor).", schema(&[], &[]),
            |_| match run("kscreen-doctor", &["-o"], None, T) {
                // strip ANSI colors
                Ok(o) if o.ok() => ToolResult::text(truncate(&strip_ansi(&o.stdout), 40_000)),
                r => out(r),
            })
        .tool("set_output", "Change monitor settings via kscreen-doctor, e.g. 'output.Virtual-1.mode.1920x1080@60' or 'output.Virtual-1.scale.1.25'.",
            schema(&[("setting", "string", "kscreen-doctor argument")], &["setting"]),
            |a| match a.str_field("setting") { Some(s) => out(run("kscreen-doctor", &[s], None, T)), None => ToolResult::error("missing 'setting'") })
        .tool("launch", "Launch an app detached from the agent, e.g. 'konsole', 'firefox', 'dolphin', 'kate'.",
            schema(&[("command", "string", "Command line")], &["command"]), launch)
        .tool("screenshot",
            "Capture the screen and return it as an image. window=<id|title|app> pins that window on top during the capture \
             and crops to it (reliable even if another window has focus). Or region 'X,Y WxH'. Fails fast if the user has Spectacle open.",
            schema(&[
                ("window", "string", "Window id, pid or title/app substring to capture"),
                ("active_window", "boolean", "Only the active window"),
                ("region", "string", "Crop 'X,Y WxH' (screen coords, e.g. from get_tree)"),
                ("scale", "number", "Image scale (default 0.5)"),
                ("save_to", "string", "PNG path"),
            ], &[]), screenshot)
        .tool("type_text", "Type text into the focused window (ydotool). Non-ASCII text is pasted through the clipboard.",
            schema(&[("text", "string", "Text"), ("paste_keys", "string", "Paste shortcut for non-ASCII (default ctrl+v; Konsole: ctrl+shift+v)")], &["text"]),
            type_text)
        .tool("key", "Press key chords, space separated: 'Return', 'Escape', 'ctrl+l', 'alt+F4', 'ctrl+shift+t', 'super'.",
            schema(&[("keys", "string", "Chord(s)")], &["keys"]), key)
        .tool("pointer", "Move the pointer to absolute screen coords (verified against KWin's real cursor and corrected) and/or click (left|middle|right|scroll_up|scroll_down).",
            schema(&[
                ("x", "integer", "X"), ("y", "integer", "Y"),
                ("button", "string", "left|middle|right|scroll_up|scroll_down"),
                ("clicks", "integer", "1-3 (default 1)"),
            ], &[]), pointer)
        .tool("clipboard_get", "Read the clipboard as text.", schema(&[], &[]), clipboard_get)
        .tool("clipboard_set", "Set the clipboard.", schema(&[("text", "string", "Text")], &["text"]),
            |a| match a.str_field("text") {
                Some(t) => out(run("sh", &["-c", "wl-copy -- \"$1\" >/dev/null 2>&1 </dev/null", "sh", t], None, T)),
                None => ToolResult::error("missing 'text'"),
            })
        .tool("notify", "Show a desktop notification.", schema(&[("summary", "string", "Title"), ("body", "string", "Body")], &["summary"]),
            |a| out(run("notify-send", &["--app-name=mcpOS", a.str_field("summary").unwrap_or("mcpOS"), a.str_field("body").unwrap_or("")], None, T)))
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() { if d.is_ascii_alphabetic() { break; } }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keycodes() {
        assert_eq!(keycode("a"), Some(30));
        assert_eq!(keycode("L"), Some(38));
        assert_eq!(keycode("ctrl"), Some(29));
        assert_eq!(keycode("Return"), Some(28));
        assert_eq!(keycode("F4"), Some(62));
        assert_eq!(keycode("F12"), Some(88));
        assert_eq!(keycode("m"), Some(50));
        assert_eq!(keycode("0"), Some(11));
        assert_eq!(keycode("nope"), None);
    }
    #[test]
    fn busctl_parse() {
        assert_eq!(busctl_strings(r#"as 3 "event0" "event1" "event12""#), vec!["event0", "event1", "event12"]);
        assert_eq!(busctl_strings(r#"s "ydotoold virtual device""#), vec!["ydotoold virtual device"]);
    }
    #[test]
    fn ansi() {
        assert_eq!(strip_ansi("\u{1b}[01;32mOutput\u{1b}[0m 1"), "Output 1");
    }
}
