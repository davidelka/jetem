#!/usr/bin/env python3
"""AI assistant plugin — explains the last command using Claude.

Press Ctrl-A i to ask Claude to explain the most recent command (why it failed,
the likely fix). Subscribes to `command_end` to remember the last command, and
shows the answer via `host/notify`.

Requirements:
  - pip install anthropic
  - export ANTHROPIC_API_KEY=...

Enable via ~/.config/terminal/plugins.toml:
    [[plugin]]
    command = "python3 /abs/path/to/examples/plugins/ai.py"
"""
import sys
import json
import threading

MODEL = "claude-opus-4-8"
SYSTEM = (
    "You are a terse shell assistant. Given a command, its output, and exit "
    "code, explain in 2-4 short lines why it failed and the most likely fix. "
    "Answer directly, no preamble."
)

_out_lock = threading.Lock()
_last = None  # last command: {"command", "output", "exit_code", "cwd"}


def send(obj):
    with _out_lock:
        sys.stdout.write(json.dumps(obj) + "\n")
        sys.stdout.flush()


def notify(text):
    send({"jsonrpc": "2.0", "id": 1, "method": "host/notify", "params": {"text": text}})


def explain_async(ctx):
    """Call Claude in a background thread and notify with the answer."""
    try:
        import anthropic
    except ImportError:
        notify("AI error: `pip install anthropic`")
        return

    cmd = ctx.get("command", "?")
    code = ctx.get("exit_code")
    cwd = ctx.get("cwd") or "?"
    output = (ctx.get("output") or "")[-4000:]
    user = f"$ {cmd}\n(exit {code}, cwd {cwd})\n\n{output}"

    try:
        client = anthropic.Anthropic()
        resp = client.messages.create(
            model=MODEL,
            max_tokens=512,
            system=SYSTEM,
            messages=[{"role": "user", "content": user}],
        )
        answer = "".join(b.text for b in resp.content if b.type == "text").strip()
        notify(answer or "(no answer)")
    except Exception as e:  # missing key, network, API error, ...
        notify(f"AI error: {e}".splitlines()[0][:160])


def handle(msg):
    global _last
    method = msg.get("method")

    if method == "initialize":
        send({
            "jsonrpc": "2.0",
            "id": msg.get("id"),
            "result": {
                "name": "ai",
                "commands": [{"id": "ai.explain", "title": "Explain last command"}],
                "keybindings": [{"keys": "prefix i", "command": "ai.explain"}],
                "events": ["command_end"],
            },
        })
    elif method == "event/command_end":
        _last = msg.get("params", {})
    elif method == "command/invoke":
        if msg.get("params", {}).get("command") == "ai.explain":
            if not _last:
                notify("nothing to explain yet")
            else:
                notify("🤖 thinking…")
                threading.Thread(target=explain_async, args=(_last,), daemon=True).start()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        handle(msg)


if __name__ == "__main__":
    main()
