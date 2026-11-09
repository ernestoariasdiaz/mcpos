"""AT-SPI helper for mcp-a11y. Invoked as:  python3 -I - '<json request>'  (script on stdin).

Uses the GObject Atspi bindings directly (at-spi2-core + python-gobject).
Prints one JSON object: {"ok": true, "text": "..."} or {"ok": false, "error": "..."}.

Node paths are child indexes from the application root, e.g. "0.3.1" = app[0][3][1].
They are stable while the UI doesn't change; re-run tree/find after big UI changes.
"""
import json
import sys
import time

try:
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi
except Exception as e:  # noqa: BLE001
    print(json.dumps({"ok": False, "error": f"AT-SPI bindings not available: {e} "
                      "(needs at-spi2-core and python-gobject)"}))
    sys.exit(0)

INTERACTIVE_ROLES = {
    "push button", "toggle button", "check box", "radio button", "menu item",
    "check menu item", "radio menu item", "entry", "password text", "combo box",
    "link", "page tab", "list item", "spin button", "slider", "text", "menu",
    "tree item", "table cell", "button", "switch", "search box", "editbar",
}
INTERESTING_STATES = ["focused", "focusable", "selected", "checked", "pressed",
                      "expanded", "collapsed", "editable", "sensitive", "enabled",
                      "showing", "visible", "busy", "invalid-entry", "required"]


def out(ok=True, text=None, error=None):
    print(json.dumps({"ok": True, "text": text} if ok else {"ok": False, "error": error}))
    sys.exit(0)


def desktop():
    return Atspi.get_desktop(0)


def children(acc):
    try:
        n = acc.get_child_count()
    except Exception:  # noqa: BLE001
        return []
    res = []
    for i in range(min(n, 5000)):
        try:
            c = acc.get_child_at_index(i)
        except Exception:  # noqa: BLE001
            c = None
        res.append(c)
    return res


def safe(fn, default=None):
    try:
        return fn()
    except Exception:  # noqa: BLE001
        return default


def states(acc):
    ss = safe(lambda: acc.get_state_set())
    if ss is None:
        return set()
    return {s.value_nick for s in safe(lambda: ss.get_states(), [])}


def ifaces(acc):
    return set(safe(lambda: acc.get_interfaces(), []) or [])


def actions(acc):
    if "Action" not in ifaces(acc):
        return []
    n = safe(lambda: Atspi.Action.get_n_actions(acc), 0) or 0
    return [safe(lambda i=i: Atspi.Action.get_action_name(acc, i), "?") for i in range(n)]


def extents(acc):
    if "Component" not in ifaces(acc):
        return None
    r = safe(lambda: Atspi.Component.get_extents(acc, Atspi.CoordType.WINDOW))
    if r is None:
        return None
    return (r.x, r.y, r.width, r.height)


def text_of(acc, limit=2000):
    if "Text" not in ifaces(acc):
        return None
    n = safe(lambda: Atspi.Text.get_character_count(acc), 0) or 0
    if n == 0:
        return ""
    t = safe(lambda: Atspi.Text.get_text(acc, 0, min(n, limit)), "")
    return t + ("…" if n > limit else "")


def value_of(acc):
    if "Value" not in ifaces(acc):
        return None
    return safe(lambda: Atspi.Value.get_current_value(acc))


def apps():
    res = []
    for i, a in enumerate(children(desktop())):
        if a is None:
            continue
        res.append((i, a))
    return res


def find_app(spec):
    spec = str(spec).strip()
    all_apps = apps()
    if spec.isdigit():
        for i, a in all_apps:
            if safe(lambda: a.get_process_id()) == int(spec):
                return a
    low = spec.lower()
    for i, a in all_apps:
        if low in (safe(lambda: a.get_name(), "") or "").lower():
            return a
    names = ", ".join(f"{safe(lambda: a.get_name(), '?')}(pid {safe(lambda: a.get_process_id(), '?')})"
                      for _, a in all_apps)
    out(False, error=f"app '{spec}' not found. Accessible apps: {names or 'none'}")


def resolve(app, path):
    node = app
    if path in (None, "", "."):
        return node
    for part in str(path).split("."):
        kids = children(node)
        idx = int(part)
        if idx >= len(kids) or kids[idx] is None:
            out(False, error=f"path {path}: index {idx} out of range (node has {len(kids)} children)")
        node = kids[idx]
    return node


def describe(acc, path, with_text=True):
    role = safe(lambda: acc.get_role_name(), "?")
    name = safe(lambda: acc.get_name(), "") or ""
    st = states(acc)
    shown = [s for s in INTERESTING_STATES if s in st and s not in ("enabled", "sensitive", "showing", "visible")]
    if "enabled" not in st and "sensitive" not in st and role in INTERACTIVE_ROLES:
        shown.append("DISABLED")
    parts = [f"[{path}]", role]
    if name:
        parts.append(json.dumps(name, ensure_ascii=False))
    desc = safe(lambda: acc.get_description(), "")
    if desc:
        parts.append(f"desc={json.dumps(desc, ensure_ascii=False)}")
    if shown:
        parts.append("{" + ",".join(shown) + "}")
    acts = actions(acc)
    if acts:
        parts.append("actions=" + ",".join(acts))
    ext = extents(acc)
    if ext and ext[2] > 0:
        parts.append("@%d,%d %dx%d" % ext)
    if with_text and role in ("entry", "text", "password text", "label", "search box", "paragraph", "heading"):
        t = text_of(acc, 200)
        if t and t != name:
            parts.append("text=" + json.dumps(t, ensure_ascii=False))
    v = value_of(acc)
    if v is not None:
        parts.append(f"value={v}")
    return " ".join(parts)


def is_interactive(acc):
    st = states(acc)
    return bool(actions(acc)) or "focusable" in st or "editable" in st


def cmd_list_apps(_):
    lines = []
    for i, a in apps():
        lines.append(f"{safe(lambda: a.get_name(), '?')}  pid={safe(lambda: a.get_process_id(), '?')}  "
                     f"windows={safe(lambda: a.get_child_count(), 0)}  toolkit={safe(lambda: a.get_toolkit_name(), '?')}")
    out(text="\n".join(lines) or "no accessible apps (is the app running? Qt needs QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1)")


def cmd_tree(r):
    app = find_app(r["app"])
    root = resolve(app, r.get("path"))
    max_depth = int(r.get("max_depth", 12))
    max_nodes = int(r.get("max_nodes", 400))
    only_inter = bool(r.get("only_interactive", False))
    include_hidden = bool(r.get("include_hidden", False))
    base = r.get("path") or ""
    lines, count = [], 0
    truncated = False

    def walk(acc, path, depth):
        nonlocal count, truncated
        if count >= max_nodes:
            truncated = True
            return
        st = states(acc)
        if not include_hidden and depth > 1 and "showing" not in st:
            return
        if not only_inter or is_interactive(acc):
            lines.append("  " * (depth if not only_inter else 0) + describe(acc, path or "."))
            count += 1
        if depth >= max_depth:
            return
        for i, c in enumerate(children(acc)):
            if c is not None:
                walk(c, f"{path}.{i}" if path else str(i), depth + 1)

    walk(root, base, 0)
    if truncated:
        lines.append(f"… truncated at {max_nodes} nodes (use path=, max_depth=, only_interactive=true)")
    out(text="\n".join(lines))


def matches(acc, r):
    if r.get("role") and safe(lambda: acc.get_role_name(), "") != r["role"]:
        return False
    if r.get("name"):
        n = (safe(lambda: acc.get_name(), "") or "").lower()
        if r["name"].lower() not in n:
            return False
    if r.get("text"):
        t = (text_of(acc, 5000) or "").lower()
        if r["text"].lower() not in t:
            return False
    if r.get("state") and r["state"] not in states(acc):
        return False
    return True


def search(r, limit):
    app = find_app(r["app"])
    res, visited = [], 0

    def walk(acc, path, depth):
        nonlocal visited
        if len(res) >= limit or visited > 20000 or depth > 40:
            return
        visited += 1
        if depth > 1 and not r.get("include_hidden") and "showing" not in states(acc):
            return
        if depth > 0 and matches(acc, r):
            res.append((path, acc))
        for i, c in enumerate(children(acc)):
            if c is not None:
                walk(c, f"{path}.{i}" if path else str(i), depth + 1)

    walk(app, "", 0)
    return res


def cmd_find(r):
    if not any(r.get(k) for k in ("role", "name", "text", "state")):
        out(False, error="give at least one of role, name, text, state")
    res = search(r, int(r.get("limit", 20)))
    out(text="\n".join(describe(a, p) for p, a in res) or "no matches")


def cmd_wait_for(r):
    deadline = time.time() + float(r.get("timeout_secs", 10))
    while True:
        res = search(r, 1)
        if res:
            p, a = res[0]
            out(text="found: " + describe(a, p))
        if time.time() >= deadline:
            out(False, error="timeout: no element matched")
        time.sleep(0.4)


def cmd_action(r):
    acc = resolve(find_app(r["app"]), r["path"])
    acts = actions(acc)
    if not acts:
        out(False, error=f"node has no actions: {describe(acc, r['path'])}")
    want = r.get("action")
    idx = 0
    if want:
        if want not in acts:
            out(False, error=f"action '{want}' not available; has: {', '.join(acts)}")
        idx = acts.index(want)
    ok = safe(lambda: Atspi.Action.do_action(acc, idx), False)
    if not ok:
        out(False, error=f"do_action '{acts[idx]}' returned false")
    out(text=f"did '{acts[idx]}' on {describe(acc, r['path'], False)}")


def cmd_focus(r):
    acc = resolve(find_app(r["app"]), r["path"])
    if "Component" not in ifaces(acc):
        out(False, error="node does not implement Component")
    if not safe(lambda: Atspi.Component.grab_focus(acc), False):
        out(False, error="grab_focus returned false")
    out(text="focused " + describe(acc, r["path"], False))


def cmd_get_text(r):
    acc = resolve(find_app(r["app"]), r["path"])
    t = text_of(acc, int(r.get("limit", 20000)))
    if t is None:
        out(False, error="node does not implement Text; name is: " + (safe(lambda: acc.get_name(), "") or ""))
    out(text=t)


def cmd_set_text(r):
    acc = resolve(find_app(r["app"]), r["path"])
    if "EditableText" not in ifaces(acc):
        out(False, error="node is not EditableText (try focus + desktop__type_text)")
    if not safe(lambda: Atspi.EditableText.set_text_contents(acc, r["text"]), False):
        out(False, error="set_text_contents returned false")
    out(text="text set")


CMDS = {
    "list_apps": cmd_list_apps, "tree": cmd_tree, "find": cmd_find, "wait_for": cmd_wait_for,
    "action": cmd_action, "focus": cmd_focus, "get_text": cmd_get_text, "set_text": cmd_set_text,
}

if __name__ == "__main__":
    try:
        req = json.loads(sys.argv[1])
        fn = CMDS[req.pop("cmd")]
        fn(req)
    except SystemExit:
        raise
    except KeyError as e:
        out(False, error=f"missing argument: {e}")
    except Exception as e:  # noqa: BLE001
        out(False, error=f"{type(e).__name__}: {e}")
