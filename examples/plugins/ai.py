#!/usr/bin/env python3
"""AI assistant plugin — explains the last command and chats about it.

Press Ctrl-A i to ask Claude about the most recent command. The answer opens in
a panel; type a follow-up there and press Enter to continue the conversation.

Two backends (set TERMINAL_AI_BACKEND=cli|api to force one):
  - "cli": shells out to the `claude` CLI — uses your Claude subscription, no key.
  - "api": the `anthropic` SDK (`pip install anthropic` + ANTHROPIC_API_KEY).
Default: api if ANTHROPIC_API_KEY is set, else the `claude` CLI if present.

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
    "code, explain why it failed and the fix in a few short lines. Answer "
    "follow-up questions directly, no preamble."
)

_out_lock = threading.Lock()
_state_lock = threading.Lock()
_last = None       # last command_end params
_convo = []        # [{"role", "content"}] sent to Claude
_transcript = []   # display lines for the panel
_busy = False


def send(obj):
    with _out_lock:
        sys.stdout.write(json.dumps(obj) + "\n")
        sys.stdout.flush()


def notify(text):
    send({"jsonrpc": "2.0", "id": 1, "method": "host/notify", "params": {"text": text}})


def show_panel(thinking=False):
    body = "\n\n".join(_transcript)
    if thinking:
        body += "\n\n🤖 …"
    send({"jsonrpc": "2.0", "id": 1, "method": "host/showPanel",
          "params": {"title": "🤖 AI  (type a follow-up, Enter to send)", "body": body, "input": True}})


# --- backends ---------------------------------------------------------------

def choose_backend():
    forced = os.environ.get("TERMINAL_AI_BACKEND")
    if forced in ("cli", "api"):
        return forced
    if os.environ.get("ANTHROPIC_API_KEY"):
        return "api"
    if shutil.which("claude"):
        return "cli"
    return "api"


def query(messages):
    """Ask Claude given the full message history."""
    if choose_backend() == "cli":
        claude = shutil.which("claude")
        if not claude:
            raise RuntimeError("`claude` CLI not found")
        prompt = ""
        for m in messages:
            who = "User" if m["role"] == "user" else "Assistant"
            prompt += f"{who}: {m['content']}\n\n"
        prompt += "Assistant:"
        proc = subprocess.run(
            [claude, "-p", prompt, "--append-system-prompt", SYSTEM],
            capture_output=True, text=True, timeout=120,
        )
        if proc.returncode != 0:
            raise RuntimeError((proc.stderr or "claude CLI failed").strip()[:160])
        return proc.stdout.strip()

    import anthropic  # ImportError handled by caller
    client = anthropic.Anthropic()
    resp = client.messages.create(model=MODEL, max_tokens=1024, system=SYSTEM, messages=messages)
    return "".join(b.text for b in resp.content if b.type == "text").strip()


def run_turn():
    """Query Claude with the current convo and append the answer."""
    global _busy
    try:
        answer = query(_convo)
    except ImportError:
        answer = "(error: `pip install anthropic`, or set TERMINAL_AI_BACKEND=cli)"
    except Exception as e:
        answer = f"(error: {str(e).splitlines()[0][:160]})"
    with _state_lock:
        _convo.append({"role": "assistant", "content": answer})
        _transcript.append("🤖 " + answer)
        _busy = False
    show_panel()


def start_explain(ctx):
    global _busy
    cmd = ctx.get("command", "?")
    code = ctx.get("exit_code")
    cwd = ctx.get("cwd") or "?"
    output = (ctx.get("output") or "")[-4000:]
    user = f"$ {cmd}\n(exit {code}, cwd {cwd})\n\n{output}"
    with _state_lock:
        _convo[:] = [{"role": "user", "content": user}]
        _transcript[:] = [f"▸ explain: {cmd}  (exit {code})"]
        _busy = True
    show_panel(thinking=True)
    threading.Thread(target=run_turn, daemon=True).start()


def start_followup(text):
    global _busy
    with _state_lock:
        if _busy:
            return
        _convo.append({"role": "user", "content": text})
        _transcript.append("you: " + text)
        _busy = True
    show_panel(thinking=True)
    threading.Thread(target=run_turn, daemon=True).start()


# --- protocol ---------------------------------------------------------------

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
    elif method == "event/panelInput":
        text = msg.get("params", {}).get("text", "").strip()
        if text:
            start_followup(text)
    elif method == "command/invoke":
        if msg.get("params", {}).get("command") == "ai.explain":
            if not _last:
                notify("nothing to explain yet")
            else:
                start_explain(_last)


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
