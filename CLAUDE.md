# CLAUDE.md — jetem

A terminal emulator in Rust, built **study-first** with David (a learning project that is
also meant to become a real, daily-usable, novel terminal). Read `docs/roadmap.md` for the
full vision/roadmap and `docs/notes.md` for the terminal-internals study notes.

## Goal & vision

Build a **plugin-first** terminal: a thin, fast, safe core where almost every feature (AI,
history, rich output, panels, themes) is a **plugin**. It should be correct enough to be a
daily driver for a **coding + SSH/sysadmin** workflow, and go *beyond* existing terminals.

**The novel angle (two reinforcing ideas):**
1. **Commands & output as first-class objects** — the *command block* (command + output +
   exit code + cwd + timestamp), captured via **OSC 133** semantic-prompt sequences (plus a
   small zsh/bash hook). Unlocks searchable **time-travel history**, **AI** over structured
   blocks, and **rich/structured output**.
2. **Plugin-first** — the core is an engine + a plugin host (event bus + registries). The AI
   assistant is "just a plugin."

**Priorities:** correctness/fidelity, UX/features, and novelty rank high. **Performance is the
lowest priority for now** — we use a CPU framebuffer; GPU is a later optimization. Don't
prematurely optimize.

## How the terminal works (data flow)

A terminal is three pieces: the **emulator** (this app), the **shell** (zsh/bash — we host
it, don't build it), and the **PTY** (kernel glue). We hold the PTY master; the shell runs on
the slave.

```
keystrokes ── encode_key ──► PTY master ──► shell + programs
window  ◄── render(grid) ◄── parser(vte) ◄── PTY master ◄── shell output (text + escape codes)
```

- Output path (reader thread): shell bytes → `vte` parser → mutate the shared `Grid` → wake
  the winit event loop → `render::paint` draws the grid into a softbuffer framebuffer.
- Input path (event loop): winit key event → `encode_key` → bytes → PTY writer → shell.
- No local echo: we send a keypress, the **shell echoes it back**, and the round trip draws it.

## Plugin model (out-of-process tier BUILT at M10; in-process tier later)

**Capability surface** a plugin can use: (1) **panels** — own a window region, run a real
program *or* custom-draw; (2) **commands + keybindings**; (3) **services** (AI, history);
(4) **event hooks** (`command_start/end`, exit codes, block events — e.g. "failed → AI");
(5) **block renderers** (rich output); (6) **theming** (colors/fonts/custom symbols).
Live today: commands+keybindings, event hooks (`command_end`, `panelInput`), and block
renderers via host actions (`host/showPanel`/`showTable`/`showTree`/`notify`/…). The protocol
is published in `docs/plugin-api.md` with a Python SDK (`sdk/jetem_plugin.py`).

**Runtime = tiered.** Out-of-process **MCP-style JSON-RPC over stdio is PRIMARY** (built — AI,
history, services — isolated, language-agnostic, fits David's MCP/Claude world). An **in-process
tier** (WASM or Rhai) is added later for custom-draw panels, glyph/theme providers, and hot-path hooks.
Native dylib plugins are ruled out (unsafe, ABI-fragile). The real design work is the
runtime-independent **event-bus + registries** layer.

### Core vs plugin: the protocol-vs-policy rule

Use this to decide what belongs in core vs. what plugins control:

- **Protocol & correctness** → **CORE** (never a plugin). What the shell/programs emit and
  expect: escape codes, colors, cursor, **alt-screen**, resize/SIGWINCH. Must behave identically
  everywhere and be fast. Plugins may *observe* these via events, but never *reimplement* them
  (a plugin "doing alt-screen its own way" would just break vim).
- **Layout, content & interaction policy** → **PLUGIN-EXTENSIBLE**. Which regions exist, what
  fills them, keybindings, themes, rich renderers.

**Surface layering** (where each concern lives):

```
WINDOW
 └─ Compositor / layout tree            ← plugins control UX HERE (M8 panes, M10 plugin panels)
     ├─ Surface (a region + a content source)
     │     • TerminalSurface = PTY + Screen{ primary ⇄ alt }   ← alt-screen is INSIDE here (core)
     │     • PaneSurface running a program (e.g. htop)         ← M8
     │     • PluginWidgetSurface (plugin draws cells)          ← later
```

Alt-screen is a detail *inside* one terminal surface; plugins operate at the surface/compositor
level *above* it, so the two never conflict. The `Surface`/compositor abstraction is introduced
at **M8** (first time there's >1 region) — not earlier, to avoid a one-implementation abstraction.

## Current code map (`src/`)

| File | Role |
|------|------|
| `main.rs` | Entry: spawn PTY, build shared `Grid`, reader thread (parse → grid → wake loop), run winit app. |
| `pty.rs` | `Pty`: spawn `$SHELL` on a PTY; `reader()`/`writer()`/`resize()`/`try_wait()`. |
| `cell.rs` | `Cell { ch, fg, bg, attrs }`, `Color` (Default/Indexed/Rgb), `attr` bit flags. |
| `grid.rs` | The screen model: cursor, deferred-wrap, erase, scroll, **scrollback + view offset**, cursor visibility. |
| `parser.rs` | `vte::Perform` impl: escape codes → grid ops (cursor moves, SGR colors, erase, `?25` cursor show/hide). |
| `font.rs` | fontdue load, cell metrics (`cell_w/cell_h/baseline`), cached glyph rasterization. |
| `render.rs` | Software painter: 256/truecolor resolve (palette from the `Theme`), alpha `blend`, glyph drawing, `draw_text`/`fill`/`draw_border` UI primitives. |
| `theme.rs` | `Theme` — all paint colors (terminal/ui/panel/recall) in one place; built-in default = the original look, overridable via `~/.config/jetem/theme.toml` (hex strings, partial). |
| `screen.rs` | `Screen{primary, alt}` — the two buffers; alt-screen switching. |
| `pane.rs` | `Rect` + `TerminalPane` (pty + screen + block tracker + reader thread); the Surface seam. |
| `layout.rs` | Binary split tree (`Layout`/`SplitDir`): `compute_rects`/`split`/`remove`. |
| `block.rs` | OSC 133 command blocks + JSONL history (`BlockTracker`); base64 command decode. |
| `recall.rs` | `Ctrl-A r` recall overlay (searchable history). |
| `panel.rs` | `TextPanel` — modal scrollable panel: wrapped text (`host/showPanel`), an aligned zebra-striped table (`host/showTable`), **or** a foldable tree (`host/showTree`); mark/copy, TSV copy for tables, arrow-key fold nav for trees. |
| `selection.rs` | Mouse text selection + extraction. |
| `plugin.rs` | **Plugin host**: JSON-RPC transport, `Registry` (chord→command→plugin), `Plugin` process. |
| `config.rs` | Plugin sources: `~/.config/jetem/plugins.toml` (explicit commands) **+** drop-in dir `~/.config/jetem/plugins/` (executable→shebang, else `.py`/`.js`/`.sh`→interpreter). |
| `window.rs` | winit `App`: compositor over panes, input/keys, prefix dispatch, host actions, toast, redraw. |

Crates: `portable-pty`, `vte`, `winit` 0.30, `softbuffer` 0.4, `fontdue`, `serde`/`serde_json`, `toml`, `arboard`, `anyhow`.
Initial **80×24** (resizable); font path hardcoded to DejaVu Sans Mono; display target **X11** (Wayland via arboard feature).

**Multiplexing is a plugin** (`examples/plugins/mux.py`) — core no longer hardcodes the split/focus/close keys. Plugins are opt-in via `plugins.toml`; the zsh integration auto-injects (no manual source).

**Writing plugins** (third-party-ready): the out-of-process protocol is fully specced in `docs/plugin-api.md` (handshake, manifest, host-action + event catalogs, chord grammar, hello-world). A Python SDK (`sdk/jetem_plugin.py`) hides the JSON-RPC plumbing — `@plug.command`/`@plug.on_event` + host-action methods. `examples/plugins/richout.py` (rich output) is built on it.

## Keybindings (cheat sheet)

Prefix = **`Ctrl-A`** (tmux-style), then a command key. Core owns only `r`/`a`; the rest are plugin-registered (opt-in via `plugins.toml`). Full table (incl. overlay/panel keys) lives in `README.md`.

- **Prefix chords** — core: `r` recall, `a` literal Ctrl-A. `mux.py`: `|`/`v` `-`/`s` split, `h`/`j`/`k`/`l` (or arrows) focus, `x` close. `ai.py`: `i` explain, `c` suggest, `m` model. `richout.py`: `t` render table/tree. `theme.py`: `y` cycle theme, `p` toggle accent.
- **Global (no prefix)** — `Ctrl-Shift-C` copy · `Ctrl-Shift-V` paste · `Shift-PageUp/Down` + wheel scroll scrollback · left-drag select.
- **Recall overlay** — type to filter, `↑`/`↓`, `Enter` insert, `Esc` close.
- **Panel** — `↑`/`↓` scroll/move, `→`/`←`/`Enter`/`Space` fold (tree), `Ctrl-Shift-C` copy, `q`/`Esc` close.

## Milestones

Done: **M1–M10** — engine, resize, alt-screen, multiplexing, command blocks + recall, and the plugin host (out-of-process JSON-RPC; multiplexing dogfooded as a plugin). **AI assistant** plugin (`examples/plugins/ai.py`): `Ctrl-A i` explains the last command, `Ctrl-A c` suggests one, `Ctrl-A m` picks the model (opus/sonnet/haiku/fable, or `JETEM_AI_MODEL`), via Claude (default `claude-opus-4-8`). Two backends (`JETEM_AI_BACKEND`): **cli** (your Claude subscription via the `claude` CLI, no key) or **api** (`anthropic` SDK + `ANTHROPIC_API_KEY`). The cli backend keeps a **persistent** `claude` process (stream-json mode), pre-warmed at load with a warm-standby per conversation, so questions answer at model speed (~5–8s) instead of cold-start speed; one-shot `claude -p` is the fallback. When touching Claude/API code, follow the `claude-api` skill.
**M11 (done): rich/structured output renderers.** Two renderers, both driven by `richout.py` (`Ctrl-A t`, *policy* in the plugin): **tables** (`host/showTable` + the `TextPanel` table mode) and a **foldable JSON tree** (`host/showTree` + a navigable tree panel mode). `richout` routes by shape — list-of-objects → table, flat dict → key/value table, nested → tree.
**M12 (done): themes.** `Theme` extraction — all paint colors in `src/theme.rs`, overridable via `~/.config/jetem/theme.toml` (hex strings, partial). Sample at `examples/theme.toml`.
**M13 (done): plugin-driven theming.** `host/setTheme` lets a plugin change the live theme at runtime (not persisted): a `preset` name swaps the whole theme, a partial JSON `patch` deep-merges onto the current one (via `Theme::patched`, serde round-trip — omitted colors keep their current value, unlike the static TOML default-fill). Built-in presets `default`/`light`/`solarized-dark` (`Theme::preset`, + user files `~/.config/jetem/themes/<name>.toml`). Demoed by `examples/plugins/theme.py` (`Ctrl-A y` cycle, `Ctrl-A p` accent-patch); SDK `set_theme(preset, patch)`.
Deferred: images (sixel/kitty), inline-in-scrollback rendering, in-process plugin tier (WASM/Rhai), `host/getTheme` + preset-picker UI, font/glyph providers. See `docs/roadmap.md`.

**Repo:** public on GitHub at https://github.com/davidelka/jetem (`main`, ssh remote `origin`). Renamed from "terminal" → "jetem". 64 unit tests passing.

## Working conventions

- **Explain before *any* change.** Before editing — not only at milestone boundaries — walk
  David through **which files will be touched and why**, file by file, and wait for his go
  *before* writing code or running `cargo add`. This is a learning project; the explanation is
  part of the point, so never skip it even for small or "obvious" changes.
- **Study-first.** This is a learning project — favor clear, well-commented code and explain the
  "why," including the real-terminal concept behind each piece.
- **Verify APIs against installed crate source** before coding (the crates' APIs shift between
  versions; grep `~/.cargo/registry/src/...` rather than guessing).
- **Milestone-based commits**, only when asked. End commit messages with the
  `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer. Currently committing to `main`.
- Keep unit tests green (`cargo test`; 64 passing). Add tests for grid/parser/panel/theme/config logic.

## Build / test / run

```bash
cargo build          # compile
cargo test           # unit tests (grid + parser)
cargo run            # launch the terminal window (needs a display; X11 here)
```

The agent's shell is headless-ish: launching the window works (exit 124 under `timeout` =
ran fine), but it can't type into it — interactive testing is David's to do.
