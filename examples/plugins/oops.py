#!/usr/bin/env python3
"""Example event-subscriber plugin.

Subscribes to `command_end` and, when a command exits non-zero, asks the host
to show a toast. Demonstrates the event bus / reaction loop (the seed of an AI
"explain this failure" assistant).

Enable via ~/.config/terminal/plugins.toml:
    [[plugin]]
    command = "python3 /abs/path/to/examples/plugins/oops.py"
"""
import sys
import json


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
                    "name": "oops",
                    "commands": [],
                    "keybindings": [],
                    "events": ["command_end"],
                },
            })
        elif method == "event/command_end":
            p = msg.get("params", {})
            code = p.get("exit_code")
            if code not in (0, None):
                cmd = p.get("command", "?")
                send({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "host/notify",
                    "params": {"text": f"  ✗ `{cmd}` exited {code}"},
                })


if __name__ == "__main__":
    main()
