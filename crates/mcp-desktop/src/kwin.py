"""KWin bridge for mcp-desktop (KDE Plasma 6, Wayland).

KWin only exposes window management to its own JavaScript scripting engine, so we:
  1. own a D-Bus name (org.mcpos.KWinBridge) with a `result(s)` method,
  2. load a tiny KWin script over D-Bus that does the work and calls back `result(json)`,
  3. wait for the callback, unload the script and print the JSON.

Invoked as:  python3 -I - '<json request>'   (script on stdin)
"""
import json
import os
import sys
import tempfile
import uuid

try:
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib
except Exception as e:  # noqa: BLE001
    print(json.dumps({"error": f"python-gobject not available: {e}"}))
    sys.exit(0)

NAME = "org.mcpos.KWinBridge"
PATH = "/org/mcpos/KWinBridge"
XML = f"""<node><interface name="{NAME}">
  <method name="result"><arg type="s" name="json" direction="in"/></method>
</interface></node>"""

JS_HEADER = r"""
function W(w) {
  var g = w.frameGeometry;
  return {
    id: w.internalId.toString(), caption: w.caption, app: w.resourceClass, pid: w.pid,
    x: g.x, y: g.y, w: g.width, h: g.height,
    active: w === workspace.activeWindow, minimized: w.minimized, fullscreen: w.fullScreen,
    maximized: (w.maximizeMode !== undefined ? w.maximizeMode === 3 : null),
    desktops: (w.desktops || []).map(function (d) { return d.name; }),
    normal: w.normalWindow, dialog: w.dialog
  };
}
function send(o) { callDBus("%(name)s", "%(path)s", "%(name)s", "result", JSON.stringify(o)); }
function find(id) {
  var l = workspace.windowList();
  for (var i = 0; i < l.length; i++) {
    var w = l[i];
    if (w.internalId.toString() === id || String(w.pid) === id) return w;
  }
  // fall back to caption / app substring
  var low = String(id).toLowerCase();
  for (var j = 0; j < l.length; j++) {
    var v = l[j];
    if ((v.normalWindow || v.dialog) &&
        ((v.caption || "").toLowerCase().indexOf(low) >= 0 || (v.resourceClass || "").toLowerCase().indexOf(low) >= 0)) return v;
  }
  return null;
}
""" % {"name": NAME, "path": PATH}

OPS = {
    "list": r"""
var ws = workspace.windowList().filter(function (w) { return w.normalWindow || w.dialog; }).map(W);
var ds = workspace.desktops.map(function (d) { return { id: d.id, name: d.name }; });
var sc = workspace.screens.map(function (s) { var g = s.geometry; return { name: s.name, x: g.x, y: g.y, w: g.width, h: g.height }; });
send({ windows: ws, desktops: ds, current: workspace.currentDesktop ? workspace.currentDesktop.name : null, screens: sc });
""",
    "window": r"""
var w = find(ARGS.id);
if (!w) { send({ error: "no window matches " + ARGS.id }); }
else {
  var a = ARGS.action;
  if (a === "activate") { if (w.minimized) w.minimized = false; workspace.activeWindow = w; }
  else if (a === "close") { w.closeWindow(); }
  else if (a === "minimize") { w.minimized = true; }
  else if (a === "unminimize") { w.minimized = false; }
  else if (a === "maximize") { w.setMaximize(true, true); }
  else if (a === "unmaximize") { w.setMaximize(false, false); }
  else if (a === "fullscreen") { w.fullScreen = !w.fullScreen; }
  else if (a === "keep_above") { w.keepAbove = !w.keepAbove; }
  else if (a === "above_on") { if (w.minimized) w.minimized = false; w.keepAbove = true; workspace.activeWindow = w; }
  else if (a === "above_off") { w.keepAbove = false; }
  else if (a === "move") {
    var g = w.frameGeometry;
    w.frameGeometry = { x: ARGS.x !== undefined ? ARGS.x : g.x, y: ARGS.y !== undefined ? ARGS.y : g.y,
                        width: ARGS.w !== undefined ? ARGS.w : g.width, height: ARGS.h !== undefined ? ARGS.h : g.height };
  }
  else if (a === "to_desktop") {
    var d = workspace.desktops.filter(function (x) { return x.name === ARGS.desktop || String(x.x11DesktopNumber) === String(ARGS.desktop); })[0];
    if (!d) { send({ error: "no desktop " + ARGS.desktop }); a = null; } else { w.desktops = [d]; }
  }
  else { send({ error: "unknown action " + a }); a = null; }
  if (a) send({ ok: true, action: a, window: W(w) });
}
""",
    "cursor": r"""
var c = workspace.cursorPos;
send({ x: Math.round(c.x), y: Math.round(c.y) });
""",
    "desktop": r"""
var d = workspace.desktops.filter(function (x) { return x.name === ARGS.desktop || String(x.x11DesktopNumber) === String(ARGS.desktop); })[0];
if (!d) send({ error: "no desktop " + ARGS.desktop });
else { workspace.currentDesktop = d; send({ ok: true, current: d.name }); }
""",
}


def main():
    req = json.loads(sys.argv[1])
    op = req.pop("op")
    if op not in OPS:
        return {"error": f"unknown op {op}"}
    timeout_ms = int(req.pop("timeout_ms", 5000))

    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    loop = GLib.MainLoop()
    state = {"acquired": False, "result": None, "lost": False}

    node = Gio.DBusNodeInfo.new_for_xml(XML)

    def on_call(conn, sender, path, iface, method, params, invocation):
        state["result"] = params.unpack()[0]
        invocation.return_value(None)
        loop.quit()

    reg = bus.register_object(PATH, node.interfaces[0], on_call, None, None)

    def acquired(*_):
        state["acquired"] = True
        loop.quit()

    def lost(*_):
        state["lost"] = True
        loop.quit()

    owner = Gio.bus_own_name_on_connection(bus, NAME, Gio.BusNameOwnerFlags.REPLACE | Gio.BusNameOwnerFlags.ALLOW_REPLACEMENT,
                                           acquired, lost)
    t1 = GLib.timeout_add(3000, loop.quit)
    loop.run()
    GLib.source_remove(t1) if not state["lost"] and state["acquired"] else None
    if not state["acquired"]:
        return {"error": "could not own D-Bus name " + NAME}

    script = JS_HEADER + "var ARGS = " + json.dumps(req) + ";\n" + OPS[op]
    fd, js = tempfile.mkstemp(prefix="mcpos-kwin-", suffix=".js")
    with os.fdopen(fd, "w") as f:
        f.write(script)
    plugin = "mcpos_" + uuid.uuid4().hex[:12]

    def kwin(path, iface, method, sig=None, args=None):
        return bus.call_sync("org.kde.KWin", path, iface, method,
                             GLib.Variant(sig, args) if sig else None, None,
                             Gio.DBusCallFlags.NONE, 5000, None)

    try:
        sid = kwin("/Scripting", "org.kde.kwin.Scripting", "loadScript", "(ss)", (js, plugin)).unpack()[0]
        if sid < 0:
            return {"error": "KWin refused to load the script"}
        ran = False
        for p in (f"/Scripting/Script{sid}", f"/{sid}"):  # Plasma 6, then Plasma 5
            try:
                kwin(p, "org.kde.kwin.Script", "run")
                ran = True
                break
            except Exception:  # noqa: BLE001
                continue
        if not ran:
            return {"error": "could not run KWin script (is this KDE Plasma on Wayland?)"}
        if state["result"] is None:
            t2 = GLib.timeout_add(timeout_ms, loop.quit)
            loop.run()
            if state["result"] is not None:
                GLib.source_remove(t2)
        if state["result"] is None:
            return {"error": "KWin script did not answer (timeout)"}
        return json.loads(state["result"])
    except GLib.Error as e:
        return {"error": f"D-Bus: {e.message}"}
    finally:
        try:
            kwin("/Scripting", "org.kde.kwin.Scripting", "unloadScript", "(s)", (plugin,))
        except Exception:  # noqa: BLE001
            pass
        os.unlink(js)
        Gio.bus_unown_name(owner)
        bus.unregister_object(reg)


if __name__ == "__main__":
    try:
        print(json.dumps(main()))
    except Exception as e:  # noqa: BLE001
        print(json.dumps({"error": f"{type(e).__name__}: {e}"}))
