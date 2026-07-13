//! A small glossary of the control bytes and escape sequences we *send* to the
//! shell, named by the key you press with the standard ANSI name in the comment.
//!
//! Terminals speak in bytes: a few single-byte C0 control codes, plus multi-byte
//! escape sequences that (almost) all start with `ESC [` — the "Control Sequence
//! Introducer" (CSI). Spelling those out as `0x1b` / `b"\x1b[A"` at every call
//! site is what makes terminal code cryptic; these names put the meaning up front.
//!
//! Convention here: **single control bytes are `u8`**, **multi-byte sequences are
//! `&str`** (all ASCII, so `.as_bytes()` gives the wire bytes). This is the
//! *outgoing* side only — incoming bytes are handled by the `vte` parser, which
//! hands us clean callbacks instead of raw escapes.

// --- single-byte C0 control codes -------------------------------------------

/// Ctrl-A (0x01) — SOH. We send this as a literal when the user presses the
/// `Ctrl-A` prefix twice (prefix, then `a`).
pub const CTRL_A: u8 = 0x01;
/// Escape (0x1b) — ESC. Sent for the Escape key, and starts every CSI sequence.
pub const ESC: u8 = 0x1b;
/// Enter (0x0d) — CR. Terminals send carriage-return, not line-feed.
pub const CR: u8 = b'\r';
/// Tab (0x09) — HT.
pub const TAB: u8 = b'\t';
/// Space (0x20).
pub const SPACE: u8 = b' ';
/// Backspace (0x7f) — DEL. What readline expects (not 0x08).
pub const BACKSPACE: u8 = 0x7f;

// --- multi-byte escape sequences (each begins with ESC `[`) ------------------

/// Cursor keys (CSI A/B/C/D). These are the "normal" (non-application) forms.
pub const CURSOR_UP: &str = "\x1b[A";
pub const CURSOR_DOWN: &str = "\x1b[B";
pub const CURSOR_RIGHT: &str = "\x1b[C";
pub const CURSOR_LEFT: &str = "\x1b[D";
/// Home / End (CSI H / F).
pub const HOME: &str = "\x1b[H";
pub const END: &str = "\x1b[F";

/// Bracketed-paste markers (CSI 200~ / 201~): when a program enables `?2004`, we
/// wrap pasted text in these so it's treated as one inert block, not typing.
pub const PASTE_START: &str = "\x1b[200~";
pub const PASTE_END: &str = "\x1b[201~";

/// Mouse-event prefixes. SGR (`ESC[<`) writes decimal coordinates; the legacy
/// X10 form (`ESC[M`) packs each field into one offset byte. See `encode_mouse`.
pub const MOUSE_SGR: &str = "\x1b[<";
pub const MOUSE_X10: &str = "\x1b[M";
