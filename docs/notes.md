# Terminal emulator — study notes

These are my own-words notes for Phase 0. Goal: understand the model before writing code.

## 1. A "terminal" is three separate things

| Piece | What it does | Who builds it |
|-------|--------------|---------------|
| **Terminal emulator** | The GUI app. Draws a grid of character cells, captures keyboard/mouse, parses bytes coming back. | **Us** |
| **Shell** (`zsh`, `bash`) | Interprets commands, runs programs, expands `$VARS`. Runs *inside* the terminal. | Not us — we host whatever exists |
| **PTY** (pseudo-terminal) | Kernel device pair that connects the two. | The OS |

The emulator does NOT understand `ls` or `cd`. It only moves bytes and draws characters.

## 2. The data flow (the most important diagram)

```
            keystrokes (bytes)
 ┌──────────────┐   e.g. "ls\r"     ┌────────┐  ┌──────────────┐
 │  Emulator    │ ────────────────► │  PTY   │ ►│ Shell + child│
 │  (our app)   │                   │ master │  │ programs     │
 │ render+parse │ ◄──────────────── │  end   │ ◄│ (vim, htop)  │
 └──────────────┘   text + escape   └────────┘  └──────────────┘
                    codes (bytes)
```

- We open a PTY and hold the **master** end.
- We spawn the shell with its stdin/stdout/stderr wired to the **slave** end.
- We write keystrokes to master → kernel delivers to shell's stdin.
- Shell writes output to its stdout → kernel delivers to master → we read & parse it.

## 3. PTY mechanics

- `openpty()` (or `portable-pty` in Rust) creates the master/slave pair.
- The slave looks like a real terminal (`/dev/pts/N`) to the shell, so the shell enables
  line editing, job control, `isatty()` returns true, programs use colors, etc.
- **Window size matters**: we must tell the kernel the grid size via `TIOCSWINSZ` (rows, cols,
  pixel w/h). On change the kernel sends `SIGWINCH` to the shell so `vim`/`htop` re-layout.
  In `portable-pty` this is `PtySize { rows, cols, .. }` + `resize()`.
- The PTY also does some processing itself (the "line discipline"): e.g. it can echo input and
  translate `\r`↔`\n`. For a raw emulator we mostly care that *we* read the program's output.

## 4. The byte stream is text + escape sequences

Output is mostly printable UTF-8, but interleaved with **control sequences** that tell us to
move the cursor, change colors, clear regions, etc. Most start with the ESC byte `0x1b`.

### Control bytes (single byte, C0 set)
- `\n` (0x0A) line feed — cursor down one row (and, with the PTY's translation, to col 0).
- `\r` (0x0D) carriage return — cursor to column 0.
- `\t` (0x09) tab — advance to next tab stop (every 8 cols).
- `\b` (0x08) backspace — cursor left one.
- `\a` (0x07) bell.

### CSI sequences — `ESC [` … final byte  (the workhorse)
Form: `ESC [` then numeric params separated by `;` then a final letter.

| Sequence | Name | Meaning |
|----------|------|---------|
| `ESC[<n>A/B/C/D` | CUU/CUD/CUF/CUB | cursor up/down/right/left n |
| `ESC[<row>;<col>H` | CUP | move cursor to row;col (1-based) |
| `ESC[<n>J` | ED | erase in display (0=to end, 1=to start, 2=all) |
| `ESC[<n>K` | EL | erase in line (0=to end, 1=to start, 2=all) |
| `ESC[<n>m` | SGR | set graphic rendition (colors/attrs) — see below |
| `ESC[?25l` / `?25h` | DECTCEM | hide / show cursor |
| `ESC[?1049h` / `l` | | enter / leave alternate screen (full-screen apps) |
| `ESC[<n>;<m>r` | DECSTBM | set scroll region |

### SGR (colors & attributes) — `ESC[ … m`
Params, semicolon-separated:
- `0` reset, `1` bold, `3` italic, `4` underline, `7` reverse.
- `30–37` fg color, `40–47` bg color (the 8 base ANSI colors). `90–97`/`100–107` bright.
- `38;5;<n>` / `48;5;<n>` = 256-color. `38;2;<r>;<g>;<b>` / `48;2;…` = 24-bit truecolor.
- Example: `\x1b[1;31m` = bold red text; `\x1b[0m` = reset.

## 5. Why we use the `vte` crate instead of hand-rolling the parser

The byte-level state machine (ground / escape / CSI-param / OSC-string states, handling
incomplete sequences split across reads) is fiddly and fully specified by Paul Williams' VT500
state diagram. `vte` implements exactly that and calls back into our `Perform` trait:

- `print(c)` — a printable char arrived → write to grid at cursor, advance cursor.
- `execute(byte)` — a C0 control byte (`\n`, `\r`, `\t`, …).
- `csi_dispatch(params, intermediates, ignore, action)` — a full CSI sequence (cursor/SGR/erase).
- `esc_dispatch(...)` — non-CSI escape sequences.
- `osc_dispatch(...)` — OSC strings, e.g. set window title `ESC ] 0 ; title BEL`.

So **we** own the meaning (grid mutations); `vte` owns the tokenizing. That's the learning
sweet spot: we implement every escape we care about, but don't re-debug the tokenizer.

## 6. The grid model (our core data structure)

- `Cell { ch: char, fg: Color, bg: Color, attrs: flags }`
- `Grid` = `rows × cols` of cells + a `Cursor { row, col }` + current pen (active fg/bg/attrs).
- Printing writes the cell under the cursor with the current pen, then advances the cursor;
  at end-of-line it wraps; at bottom it scrolls (push top line into scrollback).
- Rendering = walk the grid, draw each cell's glyph in its fg over its bg.

## 7. Build order (milestones)

1. **M1** PTY echo (no GUI) — prove the plumbing.
2. **M2** grid + `vte` parser (no GUI) — real terminal *model*, dumped as text.
3. **M3** `winit` window + `fontdue`/`softbuffer` render — *see* output.
4. **M4** keyboard → PTY — interactive; this is a real terminal.
5. **M5+** colors, cursor, scrollback, resize, config, copy/paste, then GPU/perf.

## References to read
- VT100 / xterm "ctlseqs" control-sequence reference.
- `st` (suckless terminal, ~2k lines of C) — whole pipeline at a glance.
- Alacritty's `alacritty_terminal` crate — idiomatic Rust grid + `vte` usage.
- Paul Williams' VT500-series parser state diagram (what `vte` implements).
