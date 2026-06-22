#!/usr/bin/env python3
"""AI assistant plugin — explains the last command and chats about it.

Press Ctrl-A i to ask Claude about the most recent command. The answer opens in
a panel; type a follow-up there and press Enter to continue the conversation.
Ctrl-A c translates a natural-language request into a shell command.

Two backends (set TERMINAL_AI_BACKEND=cli|api to force one):
  - "cli": drives the `claude` CLI — uses your Claude subscription, no key. To
    avoid paying process startup (~7s) on every question, the cli backend keeps a
    **persistent** `claude` process alive in stream-json mode and pre-warms it at
    load, so questions answer at model speed instead of cold-start speed.
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
_convo = []        # [{"role", "content"}] — display + the api backend's history
_transcript = []   # display lines for the panel
_busy = False
_mode = "chat"     # "chat" or "suggest"

# Persistent cli-backend sessions (lazily warmed; see ClaudeSession).
_session_lock = threading.Lock()
_active_chat = None     # session for the current explain/chat conversation
_chat_standby = None    # background-warmed spare, swapped in for a new conversation
_suggest_session = None # session for NL->command translation


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


# Persistent stream-json invocation. --strict-mcp-config skips the user's MCP
# servers (which dominate startup); stream-json needs --verbose. The system
# prompt is fixed for the process's lifetime, hence one session per prompt.
_STREAM_ARGS = [
    "claude", "-p",
    "--input-format", "stream-json",
    "--output-format", "stream-json",
    "--verbose", "--strict-mcp-config",
    "--append-system-prompt",
]


class ClaudeSession:
    """A long-lived `claude` process driven in stream-json mode. Claude keeps the
    conversation across `.ask()` calls, so the costly startup is paid once (and
    can be paid ahead of time with `.warm()`), not per question."""

    def __init__(self, system):
        self._system = system
        self._proc = None
        self._lock = threading.Lock()

    def _spawn(self):
        return subprocess.Popen(
            _STREAM_ARGS + [self._system],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, bufsize=1,
        )

    def _ensure(self):
        if self._proc is None or self._proc.poll() is not None:
            self._proc = self._spawn()

    def warm(self):
        """Start the process now (pay cold start off the hot path)."""
        with self._lock:
            self._ensure()

    def ask(self, text):
        """Send one user turn; read events until the turn's result; return it."""
        with self._lock:
            self._ensure()
            self._proc.stdin.write(
                json.dumps({"type": "user", "message": {"role": "user", "content": text}}) + "\n"
            )
            self._proc.stdin.flush()
            for line in self._proc.stdout:
                line = line.strip()
                if not line:
                    continue
                ev = json.loads(line)
                if ev.get("type") == "result":
                    if ev.get("subtype") != "success":
                        raise RuntimeError(ev.get("result") or "claude error")
                    return (ev.get("result") or "").strip()
            raise RuntimeError("claude session ended")

    def close(self):
        with self._lock:
            if self._proc:
                try:
                    self._proc.stdin.close()
                except Exception:
                    pass
                self._proc.terminate()
                self._proc = None


def prewarm():
    """Pay claude's cold start in the background at load, so the first question is
    fast. Warms the chat standby and the suggest session concurrently. No-op
    unless the cli backend is in use."""
    if choose_backend() != "cli" or not shutil.which("claude"):
        return
    threading.Thread(target=_warm_standby, daemon=True).start()
    threading.Thread(target=_warm_suggest, args=(None,), daemon=True).start()


def new_chat():
    """Swap a warm session in for a fresh conversation (matching the per-explain
    reset), retire the old one, and warm a replacement standby in the background."""
    global _active_chat, _chat_standby
    with _session_lock:
        spare, _chat_standby = _chat_standby, None
    if spare is None:
        spare = ClaudeSession(SYSTEM)
    old, _active_chat = _active_chat, spare
    if old:
        old.close()
    threading.Thread(target=_warm_standby, daemon=True).start()
    spare.warm()  # in case the standby wasn't ready yet
    return spare


def _warm_standby():
    global _chat_standby
    s = ClaudeSession(SYSTEM)
    s.warm()
    with _session_lock:
        _chat_standby = s


def _warm_suggest(old):
    """Warm a suggest session (retiring `old`, if any), so each translation is
    independent and the next one is already warm."""
    global _suggest_session
    if old:
        old.close()
    s = ClaudeSession(COMMAND_SYSTEM)
    s.warm()
    with _session_lock:
        _suggest_session = s


def _oneshot(prompt, system):
    """Stateless `claude -p` fallback (text mode) if a persistent session dies."""
    claude = shutil.which("claude")
    if not claude:
        raise RuntimeError("`claude` CLI not found")
    proc = subprocess.run(
        [claude, "-p", prompt, "--append-system-prompt", system, "--strict-mcp-config"],
        capture_output=True, text=True, timeout=120,
    )
    if proc.returncode != 0:
        raise RuntimeError((proc.stderr or "claude CLI failed").strip()[:160])
    return proc.stdout.strip()


def _flatten(messages):
    """Render the message history as a single prompt (for the one-shot fallback)."""
    out = ""
    for m in messages:
        who = "User" if m["role"] == "user" else "Assistant"
        out += f"{who}: {m['content']}\n\n"
    return out + "Assistant:"


def query_api(messages, system):
    """The api backend: one Anthropic SDK call (no process; needs a key)."""
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
    """Answer the latest user turn and append it to the transcript."""
    global _busy
    try:
        if choose_backend() == "cli":
            user_text = _convo[-1]["content"]
            try:
                answer = _active_chat.ask(user_text)
            except Exception:
                answer = _oneshot(_flatten(_convo), SYSTEM)  # resilient fallback
        else:
            answer = query_api(_convo, SYSTEM)
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
    if choose_backend() == "cli":
        new_chat()  # a fresh, warm session for this conversation
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
        if choose_backend() == "cli":
            session = _suggest_session
            try:
                raw = session.ask(request) if session else _oneshot(request, COMMAND_SYSTEM)
            except Exception:
                raw = _oneshot(request, COMMAND_SYSTEM)
            finally:
                # Keep each translation independent and the next one warm.
                threading.Thread(target=_warm_suggest, args=(session,), daemon=True).start()
        else:
            raw = query_api([{"role": "user", "content": request}], COMMAND_SYSTEM)
        cmd = clean_command(raw)
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
                "protocolVersion": 1,
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
        # Pay claude's cold start in the background, so the first question is fast.
        threading.Thread(target=prewarm, daemon=True).start()
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
