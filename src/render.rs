//! Software renderer: paints the [`Grid`] into a flat `u32` framebuffer
//! (`0x00RRGGBB` per pixel, what softbuffer expects). For each cell we fill the
//! background, then blend the glyph's coverage bitmap on top in the fg color.

use crate::cell::{attr, Color};
use crate::font::Font;
use crate::grid::Grid;
use crate::pane::Rect;

type Rgb = (u8, u8, u8);

/// Terminal defaults (a dark theme); these back `Color::Default`.
const DEFAULT_FG: Rgb = (0xcc, 0xcc, 0xcc);
const DEFAULT_BG: Rgb = (0x10, 0x12, 0x18);

/// The classic 16 ANSI colors (VGA-ish palette), indices 0–15.
const PALETTE: [Rgb; 16] = [
    (0x00, 0x00, 0x00), // 0 black
    (0xaa, 0x00, 0x00), // 1 red
    (0x00, 0xaa, 0x00), // 2 green
    (0xaa, 0x55, 0x00), // 3 yellow/brown
    (0x00, 0x00, 0xaa), // 4 blue
    (0xaa, 0x00, 0xaa), // 5 magenta
    (0x00, 0xaa, 0xaa), // 6 cyan
    (0xaa, 0xaa, 0xaa), // 7 white/grey
    (0x55, 0x55, 0x55), // 8 bright black
    (0xff, 0x55, 0x55), // 9 bright red
    (0x55, 0xff, 0x55), // 10 bright green
    (0xff, 0xff, 0x55), // 11 bright yellow
    (0x55, 0x55, 0xff), // 12 bright blue
    (0xff, 0x55, 0xff), // 13 bright magenta
    (0x55, 0xff, 0xff), // 14 bright cyan
    (0xff, 0xff, 0xff), // 15 bright white
];

/// Resolve a logical [`Color`] to concrete RGB.
fn resolve(c: Color, default: Rgb) -> Rgb {
    match c {
        Color::Default => default,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) if (i as usize) < 16 => PALETTE[i as usize],
        Color::Indexed(i) => xterm_256(i),
    }
}

/// xterm's 256-color map for indices 16–255 (6×6×6 cube, then a grey ramp).
fn xterm_256(i: u8) -> Rgb {
    if i < 232 {
        let i = i - 16;
        let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        (step(i / 36), step((i / 6) % 6), step(i % 6))
    } else {
        let v = 8 + (i - 232) * 10;
        (v, v, v)
    }
}

fn pack((r, g, b): Rgb) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Blend `fg` over an existing packed pixel by `coverage` (0–255).
fn blend(fg: Rgb, dst: u32, coverage: u8) -> u32 {
    let a = coverage as u32;
    let inv = 255 - a;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    let r = (fg.0 as u32 * a + dr * inv) / 255;
    let g = (fg.1 as u32 * a + dg * inv) / 255;
    let b = (fg.2 as u32 * a + db * inv) / 255;
    (r << 16) | (g << 8) | b
}

/// Paint `grid` into the `rect` sub-region of `buf` (a `width * height`
/// framebuffer). The caller clears any gaps between panes.
pub fn paint(buf: &mut [u32], width: usize, height: usize, rect: Rect, grid: &Grid, font: &mut Font) {
    let (cw, ch_, base) = (font.cell_w, font.cell_h, font.baseline);

    // Clear this pane's background.
    fill_rect(buf, width, height, rect.x, rect.y, rect.w, rect.h, pack(DEFAULT_BG));

    // The block cursor is only shown on the live screen (not while scrolled
    // into history) and only when the program hasn't hidden it.
    let show_cursor = grid.view_offset() == 0 && grid.cursor_visible();

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let cell = grid.visible_cell(row, col);
            let bold = cell.attrs & attr::BOLD != 0;
            let mut fg = resolve(cell.fg, DEFAULT_FG);
            let mut bg = resolve(cell.bg, DEFAULT_BG);

            // Bold + a base ANSI color (0–7) conventionally renders bright (8–15).
            if bold {
                if let Color::Indexed(i) = cell.fg {
                    if i < 8 {
                        fg = PALETTE[(i + 8) as usize];
                    }
                }
            }

            // Reverse video and the block cursor both swap fg/bg.
            let is_cursor = show_cursor && row == grid.cursor_row && col == grid.cursor_col;
            if (cell.attrs & attr::REVERSE != 0) ^ is_cursor {
                std::mem::swap(&mut fg, &mut bg);
            }

            let x0 = rect.x + col * cw;
            let y0 = rect.y + row * ch_;
            fill_rect(buf, width, height, x0, y0, cw, ch_, pack(bg));

            if cell.ch != ' ' {
                draw_glyph(buf, width, height, font, cell.ch, x0, y0, base, fg);
            }
        }
    }
}

fn fill_rect(buf: &mut [u32], width: usize, height: usize, x: usize, y: usize, w: usize, h: usize, color: u32) {
    for py in y..(y + h).min(height) {
        let start = py * width + x;
        let end = (start + w).min(py * width + width);
        buf[start..end].fill(color);
    }
}

/// Rasterize `ch` and blend its coverage into the cell at (x0, y0).
fn draw_glyph(buf: &mut [u32], width: usize, height: usize, font: &mut Font, ch: char, x0: usize, y0: usize, baseline: usize, fg: Rgb) {
    let glyph = font.glyph(ch);
    let m = &glyph.metrics;
    if m.width == 0 || m.height == 0 {
        return;
    }
    // fontdue: xmin is the left bearing; ymin is the bottom edge relative to the
    // baseline (positive = above). Convert to a top-left origin in screen space.
    let gx = x0 as i32 + m.xmin;
    let gy = y0 as i32 + baseline as i32 - m.ymin - m.height as i32;

    for by in 0..m.height {
        for bx in 0..m.width {
            let cov = glyph.coverage[by * m.width + bx];
            if cov == 0 {
                continue;
            }
            let px = gx + bx as i32;
            let py = gy + by as i32;
            if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                continue;
            }
            let idx = py as usize * width + px as usize;
            buf[idx] = blend(fg, buf[idx], cov);
        }
    }
}
