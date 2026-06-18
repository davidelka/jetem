#!/usr/bin/env python3
"""A tiny example plugin for the terminal's plugin host.

It speaks newline-delimited JSON-RPC 2.0 over stdin/stdout:
  - on `initialize`, it registers the chord `Ctrl-A g` -> command `hello.split`
  - on `command/invoke`, it asks the host to split the focused pane

Run it via ~/.config/terminal/plugins.toml:
    [[plugin]]
    command = "python3 /abs/path/to/examples/plugins/hello.py"
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
                    "name": "hello",
                    "commands": [{"id": "hello.split", "title": "Split pane"}],
                    "keybindings": [{"keys": "prefix g", "command": "hello.split"}],
                    "events": [],
                },
            })
        elif method == "command/invoke":
            if msg.get("params", {}).get("command") == "hello.split":
                send({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "host/splitPane",
                    "params": {"dir": "leftright"},
                })


if __name__ == "__main__":
    main()
