# Plugin API

How to write a plugin for **terminal** — in any language, living anywhere on disk,
with no recompilation of the core.

> Status: this documents the **current** out-of-process protocol (the M10 plugin
> host). It is not yet versioned — see [Stability & current limitations](#stability--current-limitations)
> before you ship something other people depend on.

## What a plugin is

A plugin is **an ordinary program that speaks newline-delimited JSON-RPC 2.0 over
stdin/stdout.** The terminal launches it as a child process, sends it requests on
its stdin, and reads its responses and action requests from its stdout. That's the
whole interface.

Because the boundary is a pipe, not a linked library:

- **Any language works** — Python, JavaScript, Go, Rust, a shell script. If it can
  read stdin and write stdout, it can be a plugin.
- **It lives anywhere** — your home directory, a git repo, `/opt`. Nothing has to
  be added to the terminal's source tree. (The bundled `examples/plugins/*.py` are
  there only as examples.)
- **No rebuild** — you never touch or recompile the terminal to add, change, or
  remove a plugin.

This is deliberate: native dynamic-library plugins were ruled out precisely because
they would force an ABI and a recompile. Isolation and language-independence are
the point.

What a plugin can do is fixed by the **capability surface** the host exposes:
register [commands + keybindings](#manifest), subscribe to [events](#events), and
request [host actions](#host-actions). It cannot draw pixels directly or reach into
the grid — it observes via events and acts via `host/*` requests. (That split —
the core owns protocol & correctness, plugins own layout, content & interaction
policy — is the project's core-vs-plugin rule.)

## Quick start: hello world

A plugin that binds **`Ctrl-A g`** to show a toast.

**1. Write the program.** Save this anywhere, e.g. `~/plugins/hello.py`:

```python
#!/usr/bin/env python3
"""Hello-world plugin: Ctrl-A g -> a toast."""
import sys, json

def send(obj):
    # One JSON object per line, flushed immediately.
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:                      # blocks until the host sends a line
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")

    if method == "initialize":
        # Reply to the handshake with our manifest: what we register.
        send({"jsonrpc": "2.0", "id": msg.get("id"), "result": {
            "name": "hello",
            "commands": [{"id": "hello.hi", "title": "Say hi"}],
            "keybindings": [{"keys": "prefix g", "command": "hello.hi"}],
        }})

    elif method == "command/invoke":
        if msg.get("params", {}).get("command") == "hello.hi":
            # Ask the host to show a toast.
            send({"jsonrpc": "2.0", "id": 1, "method": "host/notify",
                  "params": {"text": "hi 👋 from a plugin"}})
```

**2. Enable it.** Add a block to `~/.config/terminal/plugins.toml` (create the file
if it doesn't exist):

```toml
[[plugin]]
command = "python3 /home/you/plugins/hello.py"
```

The `command` is a plain `prog arg arg …` string, split on whitespace. Use an
**absolute path** — the terminal does not assume any working directory.

**3. Run the terminal and press `Ctrl-A g`.** A toast reading "hi 👋 from a plugin"
appears. You never rebuilt anything.

## Transport

- **Framing:** one JSON value per line (`\n`-delimited). Write a complete JSON
  object, then a newline, then **flush**. Unflushed output will hang — the host
  reads line by line.
- **Channels:** `stdin` = messages from the host to you; `stdout` = messages from
  you to the host. `stderr` is inherited by the terminal process, so anything you
  print there shows up in the console the terminal was launched from — that is your
  **log / debug channel** today.
- **JSON-RPC 2.0 shape:** requests carry a `method` (and usually `params`);
  responses carry a matching `id` and a `result`. Lines that don't parse as JSON
  are ignored by the host, so a stray log line on stdout won't crash it — but keep
  logs on stderr to be safe.
- **You may write at any time.** The host reads your stdout on a dedicated thread,
  so a plugin can emit a `host/*` action long after a command fired — e.g. from a
  background worker thread that called out to a slow API. If you write from
  multiple threads, guard stdout with a lock so lines don't interleave.

## Lifecycle

```
terminal launches your process
        │
        ├─►  host → plugin:  {"method":"initialize", ...}      (the handshake)
        │
   plugin → host:  {"id":…, "result": <manifest>}             (you register here)
        │
        │   …running…
        │   host → plugin:  {"method":"command/invoke", ...}   (a keybinding fired)
        │   host → plugin:  {"method":"event/<name>", ...}     (a subscribed event)
        │   plugin → host:  {"method":"host/<action>", ...}    (you request an action)
        │   host → plugin:  {"id":…, "result":{"ok":true}}     (reply to your action)
        │
plugin process exits  ──or──  stdin reaches EOF
        │
   host marks the plugin closed and removes its registrations
```

If your process dies, the host notices the broken pipe, removes your commands and
keybindings, and carries on. The terminal kills surviving plugin processes when it
exits.

## Messages the host sends you

| Method | When | Params | You should |
|---|---|---|---|
| `initialize` | once, at startup | `{"host":"terminal"}` | reply with your [manifest](#manifest) (a `result`, echoing the request `id`) |
| `command/invoke` | a keybinding or command you registered fired | `{"command":"<your command id>"}` | do the thing (this is a notification — no reply expected) |
| `event/<name>` | an [event](#events) you subscribed to occurred | event-specific (see below) | react (notification — no reply expected) |
| a `result` with `{"ok":bool}` | reply to a [host action](#host-actions) you sent | — | optional to read; most plugins ignore it |

## Messages you send the host

### The manifest

Sent exactly once, as the `result` of the `initialize` request. It declares
everything you register. All fields are optional.

```jsonc
{
  "name": "hello",                 // display name for your plugin
  "commands": [                    // named actions you can perform
    { "id": "hello.hi", "title": "Say hi" }
  ],
  "keybindings": [                 // chords that invoke a command
    { "keys": "prefix g", "command": "hello.hi" }
  ],
  "events": ["command_end"]        // event names you want delivered
}
```

| Field | Type | Meaning |
|---|---|---|
| `name` | string | Your plugin's display name. |
| `commands[].id` | string | Unique command id. Namespacing it (`hello.hi`) avoids clashes with other plugins — command ids are global. |
| `commands[].title` | string | Human-readable label (optional). |
| `keybindings[].keys` | string | A [chord](#keybindings--chords) that triggers the command. |
| `keybindings[].command` | string | The `command.id` to invoke. |
| `events` | string[] | [Event names](#events) to subscribe to. |

> Send `result` **only** for the `initialize` handshake. The host treats any line
> carrying a `result` as a manifest, so don't emit additional `result` messages.

### Host actions

To make something happen, send a request whose `method` starts with `host/`.
Include an `id` (any value — the host echoes it back in an `{"ok":bool}` reply you
can ignore). Unknown actions reply `{"ok":false}`.

| Action | Params | Effect |
|---|---|---|
| `host/notify` | `{"text": string}` | Show a transient toast along the bottom (multi-line text is supported). |
| `host/showPanel` | `{"title": string, "body": string, "input": bool}` | Open a modal scrollable text panel. With `"input": true` it's interactive: the user can type a line and press Enter, which is delivered back to you as the [`panelInput`](#events) event. |
| `host/closePanel` | `{}` | Close the panel. |
| `host/showTable` | `{"title": string, "headers": [string], "rows": [[any]]}` | Open a modal table panel: a header band over aligned, zebra-striped rows. Cell values that aren't strings are stringified; columns are sized to content and truncated with `…` to fit. Read-only; `Ctrl-Shift-C` copies the whole table as TSV. |
| `host/writeToFocusedPane` | `{"text": string}` | Type `text` into the focused shell (as if the user typed it — it is **not** auto-run; no trailing newline is added). |
| `host/splitPane` | `{"dir": "leftright" | "topbottom"}` | Split the focused pane (default `leftright`). |
| `host/focusPane` | `{"dir": "left" | "right" | "up" | "down"}` | Move focus to the neighbouring pane. |
| `host/closePane` | `{}` | Close the focused pane (closing the last one exits the terminal). |

Example — open an interactive panel:

```python
send({"jsonrpc": "2.0", "id": 1, "method": "host/showPanel",
      "params": {"title": "🤖 Ask", "body": "Type a question, Enter to send.",
                 "input": True}})
```

### Events

Subscribe by listing the name in your manifest's `events`. The host then sends you
`event/<name>` notifications.

| Event | Fires when | Params |
|---|---|---|
| `command_end` | a shell command finishes (captured via OSC 133) | `{"pane": int, "command": string, "exit_code": int|null, "cwd": string|null, "output": string}` |
| `panelInput` | the user submits a line in *your* interactive panel | `{"text": string}` |

Notes on `command_end`:

- `output` is the command's captured stdout/stderr **with escape sequences and
  carriage returns already stripped** — you get clean, `\n`-delimited text, ideal
  for parsing. It is capped (currently 64 KiB) so a flood can't grow unbounded.
- `exit_code` is `null` if the shell didn't report one.
- `panelInput` is only delivered to the plugin that opened the panel.

Example — react to a failed command:

```python
elif method == "event/command_end":
    p = msg.get("params", {})
    if p.get("exit_code") not in (0, None):
        send({"jsonrpc": "2.0", "id": 1, "method": "host/notify",
              "params": {"text": f"`{p['command']}` failed ({p['exit_code']})"}})
```

## Keybindings & chords

The terminal's prefix key is **`Ctrl-A`** (like tmux's `Ctrl-B`). A keybinding's
`keys` is a chord string:

| `keys` value | Pressed |
|---|---|
| `"prefix g"` | `Ctrl-A` then `g` |
| `"prefix |"` | `Ctrl-A` then `|` |
| `"prefix up"` / `"down"` / `"left"` / `"right"` | `Ctrl-A` then an arrow key |

The token after `prefix ` is the character winit produces for the key (already
shift-resolved, so `|` not `\`), or `up`/`down`/`left`/`right` for the arrows.

**Reserved chords** — the core keeps these, so don't bind them:

- `prefix r` — the command-recall overlay.
- `prefix a` — sends a literal `Ctrl-A` to the shell.

Command ids are global; chords are global. If two plugins register the same chord,
the last one to load wins, so namespace your ids and pick chords thoughtfully.

## Configuration

Plugins are explicit opt-in. The terminal reads `~/.config/terminal/plugins.toml`
(`$XDG_CONFIG_HOME/terminal/plugins.toml` if that's set). Each `[[plugin]]` block
has one field, `command`:

```toml
[[plugin]]
command = "python3 /abs/path/to/plugin.py"

[[plugin]]
command = "node /abs/path/to/other-plugin.js"
```

`command` is split on whitespace into a program and its arguments. There is no
shell expansion — use absolute paths, and pass configuration via the arguments or
the environment (e.g. wrap with `env FOO=bar python3 …`). Nothing runs unless it's
listed here; changes take effect on the next terminal launch.

## A second example: a `command_end` reactor

Putting events + actions together — toast every failed command (this is essentially
the bundled `oops.py`):

```python
#!/usr/bin/env python3
import sys, json

def send(o): sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()

for line in sys.stdin:
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": msg.get("id"), "result": {
            "name": "oops",
            "events": ["command_end"],          # no commands/keybindings — pure listener
        }})
    elif msg.get("method") == "event/command_end":
        p = msg.get("params", {})
        if p.get("exit_code") not in (0, None):
            send({"jsonrpc": "2.0", "id": 1, "method": "host/notify",
                  "params": {"text": f"✗ {p.get('command','?')} (exit {p['exit_code']})"}})
```

For a fuller, multi-threaded example (calling a slow API from a worker thread and
streaming results into an interactive panel), read the bundled
`examples/plugins/ai.py`.

## Stability & current limitations

Honest about where the contract is today:

- **No protocol version yet.** `initialize` sends `{"host":"terminal"}` with no
  version field. The message shapes here can still change; pin to a terminal commit
  if you need stability.
- **Errors surface only on stderr.** If your plugin crashes or misbehaves, the host
  removes it silently; the only trace is whatever you printed to stderr (visible in
  the launching console). There is no in-app error surface or `host/log` yet.
- **Manual enablement.** Plugins are added by editing `plugins.toml` by hand; there
  is no install command or drop-in plugin directory yet.
- **Fixed capability surface.** You can only do what the [host actions](#host-actions)
  and [events](#events) tables list. New capabilities require a (small) core change
  that adds a `host/*` action or an event — propose one if you hit a wall.

These are the known gaps between "works for us" and "a stranger can depend on it";
they'll tighten as the plugin surface matures.
