#!/usr/bin/env python3
"""Multiplexer plugin — the M10 dogfood.

Implements the Ctrl-A split/focus/close keybindings entirely as a plugin, by
registering chords and calling host actions. Proves the plugin host drives real
UX and that the core no longer needs the multiplexing keys hardcoded.

Enable via ~/.config/terminal/plugins.toml:
    [[plugin]]
    command = "python3 /abs/path/to/examples/plugins/mux.py"
"""
import sys
import json

# command id -> (host method, params)
COMMANDS = {
    "mux.split-lr": ("host/splitPane", {"dir": "leftright"}),
    "mux.split-tb": ("host/splitPane", {"dir": "topbottom"}),
    "mux.close": ("host/closePane", {}),
    "mux.focus-left": ("host/focusPane", {"dir": "left"}),
    "mux.focus-right": ("host/focusPane", {"dir": "right"}),
    "mux.focus-up": ("host/focusPane", {"dir": "up"}),
    "mux.focus-down": ("host/focusPane", {"dir": "down"}),
}

# chord -> command id
KEYBINDINGS = [
    ("prefix |", "mux.split-lr"), ("prefix v", "mux.split-lr"),
    ("prefix -", "mux.split-tb"), ("prefix s", "mux.split-tb"),
    ("prefix x", "mux.close"),
    ("prefix h", "mux.focus-left"), ("prefix left", "mux.focus-left"),
    ("prefix l", "mux.focus-right"), ("prefix right", "mux.focus-right"),
    ("prefix k", "mux.focus-up"), ("prefix up", "mux.focus-up"),
    ("prefix j", "mux.focus-down"), ("prefix down", "mux.focus-down"),
]


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue

        method = msg.get("method")

        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": msg.get("id"),
                "result": {
                    "name": "mux",
                    "commands": [{"id": cid, "title": cid} for cid in COMMANDS],
                    "keybindings": [{"keys": k, "command": c} for k, c in KEYBINDINGS],
                    "events": [],
                },
            })
        elif method == "command/invoke":
            cid = msg.get("params", {}).get("command")
            action = COMMANDS.get(cid)
            if action:
                host_method, params = action
                send({"jsonrpc": "2.0", "id": 1, "method": host_method, "params": params})


if __name__ == "__main__":
    main()
