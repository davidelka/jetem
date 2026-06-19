#!/usr/bin/env python3
"""AI assistant plugin — explains the last command using Claude.

Press Ctrl-A i to ask Claude to explain the most recent command (why it failed,
the likely fix). Subscribes to `command_end` to remember the last command, and
shows the answer via `host/notify`.

Two backends (set TERMINAL_AI_BACKEND=cli|api to force one):
  - "cli": shells out to the `claude` CLI in print mode — uses your Claude
    subscription (Pro/Max), no API key needed.
  - "api": the `anthropic` SDK (`pip install anthropic` + ANTHROPIC_API_KEY).
Default: API if ANTHROPIC_API_KEY is set, else the `claude` CLI if present.

Enable via ~/.config/terminal/plugins.toml:
    [[plugin]]
    command = "python3 /abs/path/to/examples/plugins/ai.py"
"""
import os
import sys
import json
import shutil
import subprocess
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


def choose_backend():
    forced = os.environ.get("TERMINAL_AI_BACKEND")
    if forced in ("cli", "api"):
        return forced
    if os.environ.get("ANTHROPIC_API_KEY"):
        return "api"
    if shutil.which("claude"):
        return "cli"  # use the Claude subscription via the CLI
    return "api"  # will surface a helpful error


def call_api(user):
    import anthropic  # raises ImportError if the SDK isn't installed

    client = anthropic.Anthropic()
    resp = client.messages.create(
        model=MODEL,
        max_tokens=512,
        system=SYSTEM,
        messages=[{"role": "user", "content": user}],
    )
    return "".join(b.text for b in resp.content if b.type == "text").strip()


def call_cli(user):
    """One-shot via the `claude` CLI (uses the logged-in subscription)."""
    claude = shutil.which("claude")
    if not claude:
        raise RuntimeError("`claude` CLI not found")
    proc = subprocess.run(
        [claude, "-p", user, "--append-system-prompt", SYSTEM],
        capture_output=True, text=True, timeout=120,
    )
    if proc.returncode != 0:
        raise RuntimeError((proc.stderr or "claude CLI failed").strip()[:160])
    return proc.stdout.strip()


def explain_async(ctx):
    """Call Claude in a background thread and notify with the answer."""
    cmd = ctx.get("command", "?")
    code = ctx.get("exit_code")
    cwd = ctx.get("cwd") or "?"
    output = (ctx.get("output") or "")[-4000:]
    user = f"$ {cmd}\n(exit {code}, cwd {cwd})\n\n{output}"

    backend = choose_backend()
    try:
        answer = call_cli(user) if backend == "cli" else call_api(user)
        notify(answer or "(no answer)")
    except ImportError:
        notify("AI error: `pip install anthropic` (or set TERMINAL_AI_BACKEND=cli)")
    except Exception as e:  # missing key/CLI, network, API error, ...
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
