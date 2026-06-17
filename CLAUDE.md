# CLAUDE.md — terminal

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

## Plugin model (the target architecture — not built yet)

**Capability surface** a plugin can use: (1) **panels** — own a window region, run a real
program *or* custom-draw; (2) **commands + keybindings**; (3) **services** (AI, history);
(4) **event hooks** (`command_start/end`, exit codes, block events — e.g. "failed → AI");
(5) **block renderers** (rich output); (6) **theming** (colors/fonts/custom symbols).

**Runtime = tiered.** Out-of-process **MCP-style JSON-RPC over stdio is PRIMARY** (AI, history,
services — isolated, language-agnostic, fits David's MCP/Claude world). An **in-process tier**
(WASM or Rhai) is added later for custom-draw panels, glyph/theme providers, and hot-path hooks.
Native dylib plugins are ruled out (unsafe, ABI-fragile). The real design work is the
runtime-independent **event-bus + registries** layer.

## Current code map (`src/`)

| File | Role |
|------|------|
| `main.rs` | Entry: spawn PTY, build shared `Grid`, reader thread (parse → grid → wake loop), run winit app. |
| `pty.rs` | `Pty`: spawn `$SHELL` on a PTY; `reader()`/`writer()`/`resize()`/`try_wait()`. |
| `cell.rs` | `Cell { ch, fg, bg, attrs }`, `Color` (Default/Indexed/Rgb), `attr` bit flags. |
| `grid.rs` | The screen model: cursor, deferred-wrap, erase, scroll, **scrollback + view offset**, cursor visibility. |
| `parser.rs` | `vte::Perform` impl: escape codes → grid ops (cursor moves, SGR colors, erase, `?25` cursor show/hide). |
| `font.rs` | fontdue load, cell metrics (`cell_w/cell_h/baseline`), cached glyph rasterization. |
| `render.rs` | Software painter: ANSI palette + 256/truecolor resolve, alpha `blend`, glyph drawing. **Palette/colors are hardcoded here — extract a `Theme` when themes land.** |
| `window.rs` | winit `App`/`ApplicationHandler`: window + softbuffer surface, redraw, `encode_key`, mouse/keyboard scroll. |

Crates: `portable-pty`, `vte`, `winit` 0.30, `softbuffer` 0.4, `fontdue`, `anyhow`.
Currently fixed **80×24**; font path hardcoded to DejaVu Sans Mono; display target is **X11**.

## Milestones

Done: **M1** PTY · **M2** parser+grid · **M3** render · **M4** input · **M5** scrollback/cursor/colors.
Next: **M6** resize → **M7** alt-screen → **M8** panes/multiplexing → **M9** blocks+history (OSC 133)
→ **M10** plugin host → then features as plugins (AI first). See `docs/roadmap.md`.

## Working conventions

- **Explain before editing.** For each milestone, describe the design (files, crates, concepts)
  and wait for David to say go *before* writing code or running `cargo add`.
- **Study-first.** This is a learning project — favor clear, well-commented code and explain the
  "why," including the real-terminal concept behind each piece.
- **Verify APIs against installed crate source** before coding (the crates' APIs shift between
  versions; grep `~/.cargo/registry/src/...` rather than guessing).
- **Milestone-based commits**, only when asked. End commit messages with the
  `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer. Currently committing to `main`.
- Keep unit tests green (`cargo test`; 15 passing). Add tests for grid/parser logic.

## Build / test / run

```bash
cargo build          # compile
cargo test           # unit tests (grid + parser)
cargo run            # launch the terminal window (needs a display; X11 here)
```

The agent's shell is headless-ish: launching the window works (exit 124 under `timeout` =
ran fine), but it can't type into it — interactive testing is David's to do.
