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

## 8. Mouse reporting & bracketed paste (input-side DEC modes) — M14

Two things a program can turn on so it receives richer input. Both are DEC private
modes (`ESC[?<n>h` to enable, `l` to disable). The twist vs. earlier modes: the
*parser* sets them but the *input path* (the event loop) reads them — so in jetem
they live on `Screen.modes` (shared behind the same lock), not in the parser.

**Mouse reporting.** By default we consume the mouse ourselves (select text, scroll
our scrollback). When a program asks for the mouse, we stop, and instead *encode*
each event and write it to the PTY like keystrokes:
- **Tracking level** — what to report: `?1000` press+release, `?1002` adds drag
  (motion while a button is held), `?1003` adds free motion. A program picks one.
- **Encoding** — how to write coordinates: legacy X10 packs button/col/row into
  three bytes each offset by 32 (so col 224+ overflowed — the historical 223 limit);
  **SGR `?1006`** writes them as decimal text (`ESC[<Cb;Cx;Cy` + `M` press / `m`
  release), removing the limit. Every modern app uses `?1000-3` + `?1006`.
- `Cb` = button (0/1/2) OR modifier bits (shift 4, alt 8, ctrl 16), +32 for motion;
  the wheel is 64/65. Convention: **Shift bypasses reporting** so you can still
  select text inside tmux/vim.

**Bracketed paste `?2004`.** Without it, a pasted block is indistinguishable from
typing, so a shell runs each embedded newline immediately (dangerous) and vim
re-indents every line. With it on, we wrap the paste in `ESC[200~ … ESC[201~`; the
program treats the middle as one inert literal. Security note: strip any `ESC[201~`
already in the clipboard, or a crafted paste could close the bracket early and inject
a command that *does* run.

## 9. Cursor & line-editing CSIs, and text attributes — M15

Beyond the cursor moves (`A/B/C/D`), positioning (`H`), and erase (`J/K`) from the
early milestones, real programs — especially **shell line editors** (zsh/readline)
and colored prompts — lean on a handful more. Without them, editing a long command
line or redrawing a prompt smears, because the program assumes the terminal can:
- **Position absolutely:** `G` (CHA, to a column), `` ` `` (HPA, same), `d` (VPA, to a
  row), `E`/`F` (to the start of the line N below/above). Prompts jump to a column
  constantly.
- **Edit in place:** `@` (ICH, insert blanks), `P` (DCH, delete chars — shift left),
  `X` (ECH, blank in place). These let a line editor change part of a row without
  repainting it, so they must match the shell's model exactly.
- **Save/restore the cursor:** `ESC 7`/`ESC 8` (DECSC/DECRC) and the ANSI.SYS `CSI s`/`u`.
  A program parks the cursor, draws elsewhere, and returns.

We deliberately deferred **scroll regions** (`DECSTBM ?r`, `SU/SD`, `IL/DL`, `RI`): they
need region-aware scrolling in `Grid`, and the apps that use them mostly run on the
alt-screen (already handled).

**Text attributes (SGR).** The pen already tracked bold/italic/underline/reverse; M15
adds the rest of the common set: `2` faint/dim, `5` blink, `8` conceal, `9` strike-through
(and their `22/25/28/29` resets; note `22` is "normal intensity" and clears **both** bold
and dim). A CPU framebuffer can't animate, so **blink is parsed but not shown**; dim mixes
the glyph color toward its background, conceal skips the glyph, and underline/strike-through
draw a 1px rule — which also gave us real underline rendering for the first time.

## References to read
- VT100 / xterm "ctlseqs" control-sequence reference.
- `st` (suckless terminal, ~2k lines of C) — whole pipeline at a glance.
- Alacritty's `alacritty_terminal` crate — idiomatic Rust grid + `vte` usage.
- Paul Williams' VT500-series parser state diagram (what `vte` implements).
