"""Persistent AT-SPI worker for mcp-explore.

Protocol: one JSON request per line on stdin, one JSON response per line on stdout.
  {"cmd":"snapshot","pid":N}            -> {"ok":true,"found":bool,"hash":"..","nodes":n,"actions":[...]}
  {"cmd":"act","pid":N,"path":"0.2","action":"click"}
  {"cmd":"type","pid":N,"path":"0.3","text":"..."}
  {"cmd":"ping"}

The state hash covers the *structure* of what is showing (roles + normalized names), not text
content, clocks or counters: otherwise every timestamp would look like a new state ("phantom
states", the same failure mode as phantom nodes in a knowledge graph).

If the target app hangs, an AT-SPI call here blocks; the Rust side times out, kills this worker
and records a hang. That's by design.
"""
import hashlib
import json
import re
import sys

try:
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi
except Exception as e:  # noqa: BLE001
    print(json.dumps({"ok": False, "fatal": f"AT-SPI bindings not available: {e}"}), flush=True)
    sys.exit(0)

MAX_NODES = 3000
MAX_DEPTH = 40
DIGITS = re.compile(r"\d+")


def safe(fn, default=None):
    try:
        return fn()
    except Exception:  # noqa: BLE001
        return default


def norm(name):
    name = (name or "").strip()
    name = DIGITS.sub("#", name)
    return name[:40]


def find_app(pid):
    desk = Atspi.get_desktop(0)
    for i in range(safe(lambda: desk.get_child_count(), 0) or 0):
        a = safe(lambda i=i: desk.get_child_at_index(i))
        if a is not None and safe(lambda a=a: a.get_process_id()) == pid:
            return a
    return None


def states(acc):
    ss = safe(lambda: acc.get_state_set())
    return {s.value_nick for s in (safe(lambda: ss.get_states(), []) or [])} if ss else set()


def ifaces(acc):
    return set(safe(lambda: acc.get_interfaces(), []) or [])


def actions_of(acc):
    if "Action" not in ifaces(acc):
        return []
    n = safe(lambda: Atspi.Action.get_n_actions(acc), 0) or 0
    return [safe(lambda i=i: Atspi.Action.get_action_name(acc, i), "") or "" for i in range(n)]


def resolve(app, path):
    node = app
    for part in str(path).split("."):
        if part == "":
            continue
        node = safe(lambda node=node, part=part: node.get_child_at_index(int(part)))
        if node is None:
            return None
    return node


def snapshot(pid):
    app = find_app(pid)
    if app is None:
        return {"ok": True, "found": False}
    h = hashlib.sha1()
    acts, count = [], 0
    stack = [(app, "", 0)]
    while stack and count < MAX_NODES:
        acc, path, depth = stack.pop()
        count += 1
        st = states(acc)
        if depth > 1 and "showing" not in st:
            continue
        role = safe(lambda: acc.get_role_name(), "?") or "?"
        name = norm(safe(lambda: acc.get_name(), ""))
        h.update(f"{depth}|{role}|{name};".encode())
        enabled = "enabled" in st or "sensitive" in st
        a = actions_of(acc) if enabled else []
        editable = enabled and ("editable" in st) and ("EditableText" in ifaces(acc))
        if (a or editable) and depth > 0:
            acts.append({"path": path, "role": role, "name": name, "acts": [x for x in a if x], "editable": editable})
        if depth < MAX_DEPTH:
            n = safe(lambda: acc.get_child_count(), 0) or 0
            for i in reversed(range(min(n, 500))):
                c = safe(lambda i=i: acc.get_child_at_index(i))
                if c is not None:
                    stack.append((c, f"{path}.{i}" if path else str(i), depth + 1))
    return {"ok": True, "found": True, "hash": h.hexdigest()[:16], "nodes": count, "actions": acts}


def act(pid, path, action):
    app = find_app(pid)
    node = resolve(app, path) if app else None
    if node is None:
        return {"ok": False, "error": "node gone"}
    names = actions_of(node)
    if action not in names:
        return {"ok": False, "error": f"action {action} not in {names}"}
    ok = safe(lambda: Atspi.Action.do_action(node, names.index(action)), False)
    return {"ok": bool(ok)}


def type_text(pid, path, text):
    app = find_app(pid)
    node = resolve(app, path) if app else None
    if node is None or "EditableText" not in ifaces(node):
        return {"ok": False, "error": "not editable"}
    ok = safe(lambda: Atspi.EditableText.set_text_contents(node, text), False)
    return {"ok": bool(ok)}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            r = json.loads(line)
            c = r.get("cmd")
            if c == "ping":
                out = {"ok": True}
            elif c == "snapshot":
                out = snapshot(int(r["pid"]))
            elif c == "act":
                out = act(int(r["pid"]), r["path"], r["action"])
            elif c == "type":
                out = type_text(int(r["pid"]), r["path"], r["text"])
            else:
                out = {"ok": False, "error": f"unknown cmd {c}"}
        except Exception as e:  # noqa: BLE001
            out = {"ok": False, "error": f"{type(e).__name__}: {e}"}
        print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
