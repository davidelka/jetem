# jetem

A **plugin-first, AI-native terminal emulator** in Rust — where almost every feature
(AI, history, rich output, multiplexing) is a plugin over a thin, correct core.

> Built **study-first**: a learning project that is also meant to become a real,
> daily-usable terminal for a coding + SSH/sysadmin workflow. Correctness and
> novelty rank high; performance is deliberately deprioritized for now (it uses a
> CPU framebuffer — GPU is a later optimization).

## The novel angle

Two reinforcing ideas set jetem apart from a normal terminal:

1. **Commands & output as first-class objects.** Each *command block* — command +
   output + exit code + cwd + timestamp — is captured via **OSC 133** semantic-prompt
   sequences (plus a tiny zsh hook). That unlocks searchable **time-travel history**,
   **AI** that sees structured blocks, and **rich/structured output** renderers.
2. **Plugin-first.** The core is an engine + a plugin host (event bus + registries).
   Features are out-of-process plugins speaking JSON-RPC over stdio — **any language,
   no recompile**. The AI assistant is "just a plugin."

The dividing line is **protocol vs. policy**: the core owns what the shell and programs
emit and expect (escape codes, colors, alt-screen, resize) and must behave identically
everywhere; plugins own layout, content, keybindings, and renderers.

## Status

A working classic terminal (vim / tmux / less / ssh all work) plus the novel layer.
Milestones **M1–M11** done: engine, resize, alt-screen, multiplexing, command blocks +
recall, the plugin host, and the first rich-output renderer. See
[`docs/roadmap.md`](docs/roadmap.md).

## Features

- **Classic core** — `vte`-based parser, the grid model, scrollback, window resize
  (SIGWINCH), the alternate screen, truecolor/256-color, mouse selection + clipboard.
- **Command blocks + recall** — OSC 133 capture, persisted to JSONL; `Ctrl-A r` opens a
  searchable history overlay.
- **Plugin host** — out-of-process JSON-RPC (MCP-style); registries for commands,
  keybindings, events, and host actions. Fully specced in
  [`docs/plugin-api.md`](docs/plugin-api.md).
- **Bundled plugins** (in [`examples/plugins/`](examples/plugins)):
  - `mux.py` — multiplexing (splits / focus / close). Multiplexing is itself a plugin.
  - `ai.py` — `Ctrl-A i` explains the last command, `Ctrl-A c` suggests one, via Claude
    (your subscription through the `claude` CLI, or the Anthropic SDK). Keeps a
    persistent, pre-warmed `claude` process so answers come at model speed.
  - `richout.py` — `Ctrl-A t` renders the last command's output (JSON or aligned
    columns) as a table.
  - `oops.py` — toasts when a command fails.

## Build & run

Needs Rust and a display (X11; Wayland via the arboard feature). The font path is
currently hardcoded to DejaVu Sans Mono.

```bash
cargo build      # compile
cargo test       # unit tests (grid, parser, panel, config, plugin host)
cargo run        # launch the terminal window
```

Plugins are opt-in. Copy [`examples/plugins.toml`](examples/plugins.toml) to
`~/.config/jetem/plugins.toml` and fix the paths, or just **drop an executable into
`~/.config/jetem/plugins/`** and it loads automatically. The `ai.py` plugin needs the
`claude` CLI (or `pip install anthropic` + `ANTHROPIC_API_KEY`).

## Write a plugin in ~10 lines

A plugin is any program that speaks newline-delimited JSON-RPC over stdio — no rebuild,
any language. With the bundled Python SDK ([`sdk/jetem_plugin.py`](sdk/jetem_plugin.py)):

```python
from jetem_plugin import Plugin

plug = Plugin("hello")

@plug.command("hello.hi", title="Say hi", keys="prefix g")
def hi():
    plug.notify("hi 👋 from a plugin")

plug.run()
```

Drop that in `~/.config/jetem/plugins/`, restart, and press `Ctrl-A g`. The full
protocol — host actions, events, the manifest, the chord grammar — is documented in
[`docs/plugin-api.md`](docs/plugin-api.md).

## Keybindings

The prefix is **`Ctrl-A`** (like tmux's `Ctrl-B`). Core keeps only `r` and `a`; the rest
are registered by plugins.

| Chord | Action | From |
|---|---|---|
| `Ctrl-A r` | searchable command-block recall | core |
| `Ctrl-A a` | send a literal `Ctrl-A` to the shell | core |
| `Ctrl-A \|` / `v`, `-` / `s` | split pane (left-right / top-bottom) | `mux.py` |
| `Ctrl-A h` `j` `k` `l` / arrows | move focus between panes | `mux.py` |
| `Ctrl-A x` | close the focused pane | `mux.py` |
| `Ctrl-A i` | explain the last command (AI) | `ai.py` |
| `Ctrl-A c` | suggest a command (NL → shell) | `ai.py` |
| `Ctrl-A m` | pick the AI model (opus/sonnet/haiku/fable) | `ai.py` |
| `Ctrl-A t` | render last output as a table | `richout.py` |

## Layout

| Path | What |
|---|---|
| `src/` | the core engine + plugin host (see the code map in [`CLAUDE.md`](CLAUDE.md)) |
| `docs/plugin-api.md` | the plugin protocol spec + hello-world |
| `docs/roadmap.md` | vision, architecture, and milestones |
| `docs/notes.md` | terminal-internals study notes |
| `examples/plugins/` | the bundled plugins |
| `sdk/jetem_plugin.py` | the Python plugin SDK |

## Roadmap (next)

Themes (extract a `Theme` from the hardcoded palette), an in-process plugin tier
(WASM/Rhai) for custom-draw panels, and more block renderers (foldable JSON, images).
Details in [`docs/roadmap.md`](docs/roadmap.md).
