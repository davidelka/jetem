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

use serde::{Deserialize, Deserializer};

/// An RGB color. Deserializes from a `#rrggbb` (or bare `rrggbb`) hex string.
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

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Terminal {
    pub fg: Col,
    pub bg: Col,
    pub selection: Col,
    pub palette: [Col; 16],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub divider: Col,
    pub focus_border: Col,
}

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Recall {
    pub bg: Col,
    pub text: Col,
    pub dim: Col,
    pub sel_bg: Col,
    pub sel_fg: Col,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub terminal: Terminal,
    pub ui: Ui,
    pub panel: Panel,
    pub recall: Recall,
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
}

fn theme_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("theme.toml"))
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
}
