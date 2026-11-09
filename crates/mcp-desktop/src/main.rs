//! mcp-desktop: drive the graphical desktop. Picks a backend from the running session:
//!   KDE Plasma (KWin)  -> kde.rs   (default for mcpOS)
//!   Sway / wlroots     -> sway.rs
//! Override with MCP_DESKTOP_BACKEND=kde|sway.

mod kde;
mod sway;

use mcp_core::serve_stdio;

fn backend() -> &'static str {
    if let Ok(b) = std::env::var("MCP_DESKTOP_BACKEND") {
        return if b == "sway" { "sway" } else { "kde" };
    }
    let desk = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    if desk.contains("kde") || std::env::var("KDE_FULL_SESSION").is_ok() {
        "kde"
    } else if desk.contains("sway") || std::env::var("SWAYSOCK").is_ok() {
        "sway"
    } else {
        // launched outside the session (tty/ssh): guess from what's running
        let kwin = mcp_core::proc::run("pgrep", &["-x", "kwin_wayland"], None, std::time::Duration::from_secs(5))
            .map(|o| o.ok())
            .unwrap_or(false);
        if kwin { "kde" } else { "sway" }
    }
}

fn main() {
    sway::fix_env(); // sets XDG_RUNTIME_DIR / WAYLAND_DISPLAY if missing; harmless on KDE
    let b = backend();
    let (mut tools, hint) = match b {
        "kde" => (
            kde::tools(),
            "Controls the KDE Plasma desktop. Loop: get_tree -> launch / window(activate) -> screenshot -> key/type_text/pointer. \
             For reading/clicking UI elements prefer the a11y__* tools.",
        ),
        _ => (
            sway::tools(),
            "Controls the Sway desktop. Typical loop: get_tree -> launch/focus -> screenshot -> key/type_text/pointer -> screenshot.",
        ),
    };
    eprintln!("mcp-desktop: backend {b}");
    serve_stdio("mcp-desktop", env!("CARGO_PKG_VERSION"), Some(hint), &mut tools);
}
