//! The color theme: every paint color in one place, with today's look as the
//! built-in default and an optional `~/.config/jetem/theme.toml` override.
//!
//! Colors are written as hex strings in the TOML (`fg = "#cccccc"`). Sections and
//! individual keys are all optional — `#[serde(default)]` fills any you omit from
//! the default, so a theme file can override just the few colors you care about:
//!
//! ```toml
//! [terminal]
//! bg = "#101218"
//! [panel]
//! title = "#78b4fa"
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An RGB color. (De)serializes as a `#rrggbb` (or bare `rrggbb`) hex string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Col(pub u8, pub u8, pub u8);

impl Col {
    /// As an `(r, g, b)` tuple — the form the render primitives take.
    pub const fn rgb(self) -> (u8, u8, u8) {
        (self.0, self.1, self.2)
    }
    /// As a packed `0x00RRGGBB` pixel — for `fill`/`draw_border`.
    pub const fn packed(self) -> u32 {
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | self.2 as u32
    }
}

fn parse_hex(s: &str) -> Option<Col> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Col(r, g, b))
}

impl<'de> Deserialize<'de> for Col {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid hex color: {s:?}")))
    }
}

impl Serialize for Col {
    /// Emits `#rrggbb` — the exact form the hex `Deserialize` above reads back,
    /// so a theme round-trips through JSON/TOML unchanged.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Terminal {
    pub fg: Col,
    pub bg: Col,
    pub selection: Col,
    pub palette: [Col; 16],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub divider: Col,
    pub focus_border: Col,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Panel {
    pub bg: Col,
    pub title: Col,
    pub text: Col,
    pub hint: Col,
    pub sel: Col,
    pub input: Col,
    pub border: Col,
    pub header_fg: Col,
    pub header_bg: Col,
    pub stripe: Col,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recall {
    pub bg: Col,
    pub text: Col,
    pub dim: Col,
    pub sel_bg: Col,
    pub sel_fg: Col,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Search {
    /// Background tint of every scrollback-search match.
    pub match_bg: Col,
    /// Background tint of the *current* (focused) match — brighter.
    pub current_bg: Col,
    /// Foreground of the `/query (n/total)` prompt bar.
    pub prompt: Col,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub terminal: Terminal,
    pub ui: Ui,
    pub panel: Panel,
    pub recall: Recall,
    pub search: Search,
}

impl Theme {
    /// Load `~/.config/jetem/theme.toml` if present, else the built-in default. A
    /// malformed file falls back to the default (rather than failing to launch).
    pub fn load() -> Self {
        match theme_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => toml::from_str(&text).unwrap_or_default(),
            None => Self::default(),
        }
    }

    /// Return a copy of this theme with `patch` (a partial theme as JSON) merged in.
    ///
    /// This is what powers a *live* `host/setTheme` patch: unlike the static TOML
    /// path — where `#[serde(default)]` fills any omitted field from `Default` — we
    /// merge onto the **current** theme, so a patch that touches only `panel.title`
    /// leaves every other color exactly as it is. Implemented by round-tripping
    /// through `serde_json`: serialize self → deep-merge the patch object → deserialize.
    /// A patch that can't deserialize (e.g. a bad hex string) falls back to `self`.
    pub fn patched(&self, patch: &serde_json::Value) -> Self {
        let mut base = match serde_json::to_value(self) {
            Ok(v) => v,
            Err(_) => return self.clone(),
        };
        merge_json(&mut base, patch);
        serde_json::from_value(base).unwrap_or_else(|_| self.clone())
    }

    /// Look up a named preset. A user file at `~/.config/jetem/themes/<name>.toml`
    /// wins over a built-in of the same name; otherwise fall back to the built-ins.
    /// Unknown names return `None` (the caller treats that as a no-op).
    pub fn preset(name: &str) -> Option<Self> {
        if let Some(text) = preset_path(name).and_then(|p| std::fs::read_to_string(p).ok()) {
            if let Ok(t) = toml::from_str(&text) {
                return Some(t);
            }
        }
        match name {
            "default" => Some(Self::default()),
            "light" => Some(Self::light()),
            "solarized-dark" => Some(Self::solarized_dark()),
            _ => None,
        }
    }
}

/// Recursively merge `patch` into `base`, in place. Objects merge key-by-key;
/// any non-object value (or a key absent from `base`) is overwritten wholesale.
fn merge_json(base: &mut serde_json::Value, patch: &serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                merge_json(b.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (b, p) => *b = p.clone(),
    }
}

fn theme_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("theme.toml"))
}

fn preset_path(name: &str) -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("themes").join(format!("{name}.toml")))
}

// --- the built-in default (reproduces the original hardcoded look) ----------

impl Default for Terminal {
    fn default() -> Self {
        Self {
            fg: Col(0xcc, 0xcc, 0xcc),
            bg: Col(0x10, 0x12, 0x18),
            selection: Col(38, 64, 102),
            // The classic 16 ANSI colors (VGA-ish), indices 0–15.
            palette: [
                Col(0x00, 0x00, 0x00),
                Col(0xaa, 0x00, 0x00),
                Col(0x00, 0xaa, 0x00),
                Col(0xaa, 0x55, 0x00),
                Col(0x00, 0x00, 0xaa),
                Col(0xaa, 0x00, 0xaa),
                Col(0x00, 0xaa, 0xaa),
                Col(0xaa, 0xaa, 0xaa),
                Col(0x55, 0x55, 0x55),
                Col(0xff, 0x55, 0x55),
                Col(0x55, 0xff, 0x55),
                Col(0xff, 0xff, 0x55),
                Col(0x55, 0x55, 0xff),
                Col(0xff, 0x55, 0xff),
                Col(0x55, 0xff, 0xff),
                Col(0xff, 0xff, 0xff),
            ],
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            divider: Col(0x1a, 0x1a, 0x22),
            focus_border: Col(0x5a, 0x9c, 0xe6),
        }
    }
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            bg: Col(24, 26, 34),
            title: Col(120, 180, 250),
            text: Col(210, 210, 220),
            hint: Col(120, 120, 135),
            sel: Col(50, 82, 122),
            input: Col(235, 235, 245),
            border: Col(0x5a, 0x9c, 0xe6),
            header_fg: Col(150, 200, 255),
            header_bg: Col(38, 44, 60),
            stripe: Col(30, 33, 43),
        }
    }
}

impl Default for Recall {
    fn default() -> Self {
        Self {
            bg: Col(28, 28, 36),
            text: Col(205, 205, 215),
            dim: Col(120, 120, 135),
            sel_bg: Col(90, 156, 230),
            sel_fg: Col(16, 18, 24),
        }
    }
}

impl Default for Search {
    fn default() -> Self {
        Self {
            match_bg: Col(90, 74, 20),    // dim amber
            current_bg: Col(200, 160, 40), // bright amber
            prompt: Col(235, 200, 120),
        }
    }
}

// --- built-in named presets (beyond the default) ----------------------------

impl Theme {
    /// A light theme: dark text on a near-white background, ANSI palette dimmed so
    /// it stays legible on light. Every UI region is re-tinted, not just the terminal.
    pub fn light() -> Self {
        Self {
            terminal: Terminal {
                fg: Col(0x2b, 0x2b, 0x2b),
                bg: Col(0xf7, 0xf7, 0xf2),
                selection: Col(0xbe, 0xd6, 0xff),
                palette: [
                    Col(0x00, 0x00, 0x00),
                    Col(0xc0, 0x1c, 0x28),
                    Col(0x26, 0xa2, 0x69),
                    Col(0xa2, 0x73, 0x4c),
                    Col(0x12, 0x48, 0x8b),
                    Col(0xa3, 0x47, 0xba),
                    Col(0x0b, 0x8a, 0x8f),
                    Col(0x50, 0x50, 0x50),
                    Col(0x80, 0x80, 0x80),
                    Col(0xe0, 0x1b, 0x24),
                    Col(0x2e, 0xc2, 0x7e),
                    Col(0xb5, 0x89, 0x00),
                    Col(0x1a, 0x5f, 0xb4),
                    Col(0xc0, 0x61, 0xcb),
                    Col(0x10, 0xa4, 0xa8),
                    Col(0x1a, 0x1a, 0x1a),
                ],
            },
            ui: Ui {
                divider: Col(0xd8, 0xd8, 0xd0),
                focus_border: Col(0x1a, 0x5f, 0xb4),
            },
            panel: Panel {
                bg: Col(0xff, 0xff, 0xfa),
                title: Col(0x12, 0x48, 0x8b),
                text: Col(0x2b, 0x2b, 0x2b),
                hint: Col(0x8a, 0x8a, 0x8a),
                sel: Col(0xbe, 0xd6, 0xff),
                input: Col(0x10, 0x10, 0x10),
                border: Col(0x1a, 0x5f, 0xb4),
                header_fg: Col(0x0d, 0x3a, 0x73),
                header_bg: Col(0xe6, 0xe9, 0xf0),
                stripe: Col(0xef, 0xef, 0xe8),
            },
            recall: Recall {
                bg: Col(0xff, 0xff, 0xfa),
                text: Col(0x2b, 0x2b, 0x2b),
                dim: Col(0x8a, 0x8a, 0x8a),
                sel_bg: Col(0x1a, 0x5f, 0xb4),
                sel_fg: Col(0xf7, 0xf7, 0xf2),
            },
            search: Search::default(),
        }
    }

    /// Solarized Dark — Ethan Schoonover's classic base03 background with the
    /// signature accent palette.
    pub fn solarized_dark() -> Self {
        // Solarized reference values.
        let base03 = Col(0x00, 0x2b, 0x36);
        let base02 = Col(0x07, 0x36, 0x42);
        let base01 = Col(0x58, 0x6e, 0x75);
        let base0 = Col(0x83, 0x94, 0x96);
        let base1 = Col(0x93, 0xa1, 0xa1);
        let yellow = Col(0xb5, 0x89, 0x00);
        let orange = Col(0xcb, 0x4b, 0x16);
        let red = Col(0xdc, 0x32, 0x2f);
        let magenta = Col(0xd3, 0x36, 0x82);
        let violet = Col(0x6c, 0x71, 0xc4);
        let blue = Col(0x26, 0x8b, 0xd2);
        let cyan = Col(0x2a, 0xa1, 0x98);
        let green = Col(0x85, 0x99, 0x00);
        Self {
            terminal: Terminal {
                fg: base0,
                bg: base03,
                selection: base02,
                palette: [
                    base02, red, green, yellow, blue, magenta, cyan, base1, base01, orange, base01,
                    base0, base0, violet, base1, Col(0xfd, 0xf6, 0xe3),
                ],
            },
            ui: Ui {
                divider: base02,
                focus_border: blue,
            },
            panel: Panel {
                bg: base02,
                title: blue,
                text: base1,
                hint: base01,
                sel: Col(0x0e, 0x4b, 0x5a),
                input: Col(0xfd, 0xf6, 0xe3),
                border: blue,
                header_fg: cyan,
                header_bg: base03,
                stripe: base03,
            },
            recall: Recall {
                bg: base02,
                text: base1,
                dim: base01,
                sel_bg: blue,
                sel_fg: base03,
            },
            search: Search::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_with_and_without_hash() {
        assert_eq!(parse_hex("#1a1a22"), Some(Col(26, 26, 34)));
        assert_eq!(parse_hex("ffffff"), Some(Col(255, 255, 255)));
        assert_eq!(parse_hex("#zzzzzz"), None);
        assert_eq!(parse_hex("#fff"), None); // must be 6 digits
    }

    #[test]
    fn col_packs() {
        assert_eq!(Col(0x5a, 0x9c, 0xe6).packed(), 0x00_5a_9c_e6);
    }

    #[test]
    fn example_theme_toml_parses_and_matches_default() {
        // The shipped sample must stay valid and (since it mirrors the defaults)
        // round-trip to the built-in look.
        let s = include_str!("../examples/theme.toml");
        let t: Theme = toml::from_str(s).expect("examples/theme.toml must parse");
        assert_eq!(t.terminal.fg, Terminal::default().fg);
        assert_eq!(t.terminal.palette[9], Terminal::default().palette[9]);
        assert_eq!(t.recall.sel_bg, Recall::default().sel_bg);
    }

    #[test]
    fn partial_toml_overrides_only_named_keys() {
        // Only panel.title is set; everything else must stay at the default.
        let t: Theme = toml::from_str("[panel]\ntitle = \"#ff0000\"\n").unwrap();
        assert_eq!(t.panel.title, Col(0xff, 0x00, 0x00)); // overridden
        assert_eq!(t.panel.bg, Panel::default().bg); // untouched
        assert_eq!(t.terminal.fg, Terminal::default().fg); // whole section untouched
    }

    #[test]
    fn theme_round_trips_through_json() {
        // Serialize → deserialize must be the identity, so `patched` (which relies
        // on it) never silently drops or mangles a color. Covers Col's hex Serialize.
        let t = Theme::solarized_dark();
        let v = serde_json::to_value(&t).unwrap();
        let back: Theme = serde_json::from_value(v).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn patched_merges_onto_current_not_default() {
        // Start from a non-default theme, patch only panel.title. The patched color
        // must change while an unrelated field keeps the *current* value (proving we
        // merged onto self, not reset omitted keys to Default).
        let base = Theme::light();
        assert_ne!(base.terminal.bg, Theme::default().terminal.bg); // precondition
        let patch = serde_json::json!({ "panel": { "title": "#ff0000" } });
        let t = base.patched(&patch);
        assert_eq!(t.panel.title, Col(0xff, 0x00, 0x00)); // patched
        assert_eq!(t.terminal.bg, base.terminal.bg); // untouched, still light's bg
        assert_eq!(t.panel.bg, base.panel.bg); // sibling key in patched section kept
    }

    #[test]
    fn bad_patch_falls_back_to_self() {
        // An un-deserializable patch (invalid hex) must not blow away the theme.
        let base = Theme::default();
        let t = base.patched(&serde_json::json!({ "panel": { "title": "not-a-color" } }));
        assert_eq!(t, base);
    }

    #[test]
    fn preset_known_and_unknown() {
        assert!(Theme::preset("default").is_some());
        assert!(Theme::preset("light").is_some());
        assert!(Theme::preset("solarized-dark").is_some());
        assert!(Theme::preset("nope").is_none());
    }
}
