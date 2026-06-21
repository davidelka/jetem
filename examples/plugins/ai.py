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
COMMAND_SYSTEM = (
    "Translate the user's request into a single shell command for a Linux "
    "zsh shell. Output ONLY the command line — no explanation, no markdown, "
    "no backticks, no leading $."
)

_out_lock = threading.Lock()
_state_lock = threading.Lock()
_last = None       # last command_end params
_convo = []        # [{"role", "content"}] sent to Claude
_transcript = []   # display lines for the panel
_busy = False
_mode = "chat"     # "chat" or "suggest"


def send(obj):
    with _out_lock:
        sys.stdout.write(json.dumps(obj) + "\n")
        sys.stdout.flush()


def notify(text):
    send({"jsonrpc": "2.0", "id": 1, "method": "host/notify", "params": {"text": text}})


def panel(title, body, interactive=True):
    send({"jsonrpc": "2.0", "id": 1, "method": "host/showPanel",
          "params": {"title": title, "body": body, "input": interactive}})


def close_panel():
    send({"jsonrpc": "2.0", "id": 1, "method": "host/closePanel", "params": {}})


def write_to_pane(text):
    send({"jsonrpc": "2.0", "id": 1, "method": "host/writeToFocusedPane", "params": {"text": text}})


def show_panel(thinking=False):
    body = "\n\n".join(_transcript)
    if thinking:
        body += "\n\n🤖 …"
    panel("🤖 AI  (type a follow-up, Enter to send)", body)


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


def query(messages, system=SYSTEM):
    """Ask Claude given the message history and a system prompt."""
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
            [claude, "-p", prompt, "--append-system-prompt", system],
            capture_output=True, text=True, timeout=120,
        )
        if proc.returncode != 0:
            raise RuntimeError((proc.stderr or "claude CLI failed").strip()[:160])
        return proc.stdout.strip()

    import anthropic  # ImportError handled by caller
    client = anthropic.Anthropic()
    resp = client.messages.create(model=MODEL, max_tokens=1024, system=system, messages=messages)
    return "".join(b.text for b in resp.content if b.type == "text").strip()


def clean_command(text):
    """Strip markdown/prefixes so we paste a bare command line."""
    cmd = text.strip()
    if cmd.startswith("```"):
        cmd = cmd.strip("`")
        cmd = cmd.split("\n", 1)[-1] if "\n" in cmd else cmd
    # First non-empty line, minus a leading prompt char.
    for line in cmd.splitlines():
        line = line.strip()
        if line:
            return line[2:] if line.startswith("$ ") else line.lstrip("`").rstrip("`")
    return ""


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


def start_suggest():
    global _mode
    _mode = "suggest"
    panel("💡 Suggest a command", "Describe what you want to do, then press Enter.", interactive=True)


def suggest_worker(request):
    """Translate NL -> command, paste it at the prompt, close the panel."""
    global _mode
    panel("💡 Suggest a command", f"request: {request}\n\n🤖 finding a command…", interactive=False)
    try:
        cmd = clean_command(query([{"role": "user", "content": request}], system=COMMAND_SYSTEM))
    except Exception as e:
        panel("💡 Suggest a command", f"error: {str(e).splitlines()[0][:160]}", interactive=False)
        _mode = "chat"
        return
    _mode = "chat"
    if cmd:
        write_to_pane(cmd)  # inserted at the prompt, not run
    close_panel()


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
                "commands": [
                    {"id": "ai.explain", "title": "Explain last command"},
                    {"id": "ai.suggest", "title": "Suggest a command"},
                ],
                "keybindings": [
                    {"keys": "prefix i", "command": "ai.explain"},
                    {"keys": "prefix c", "command": "ai.suggest"},
                ],
                "events": ["command_end"],
            },
        })
    elif method == "event/command_end":
        _last = msg.get("params", {})
    elif method == "event/panelInput":
        text = msg.get("params", {}).get("text", "").strip()
        if not text:
            return
        if _mode == "suggest":
            threading.Thread(target=suggest_worker, args=(text,), daemon=True).start()
        else:
            start_followup(text)
    elif method == "command/invoke":
        cmd = msg.get("params", {}).get("command")
        if cmd == "ai.explain":
            if not _last:
                notify("nothing to explain yet")
            else:
                start_explain(_last)
        elif cmd == "ai.suggest":
            start_suggest()


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
