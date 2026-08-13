//! Deterministic visual captures of XTUI frames.
//!
//! The renderer uses Ratatui's `TestBackend`, then translates the exact cell
//! buffer into a standalone SVG. This keeps screenshot generation headless,
//! portable, and faithful to the styles the real terminal receives.

use super::draw;
use crate::{app::App, ui::theme};
use anyhow::{Result, bail};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Cell,
    style::{Color, Modifier},
};

/// Geometry and typography for a terminal capture.
#[derive(Clone, Copy, Debug)]
pub struct CaptureOptions {
    pub columns: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
    pub font_size: u16,
}

impl CaptureOptions {
    /// Standard landing-page review viewport (110 columns by 44 rows).
    pub const fn landing() -> Self {
        Self {
            columns: 110,
            rows: 44,
            cell_width: 9,
            cell_height: 18,
            font_size: 15,
        }
    }
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self::landing()
    }
}

/// Render the current app frame as a self-contained SVG terminal screenshot.
pub fn capture_svg(app: &mut App, options: CaptureOptions) -> Result<String> {
    if options.columns == 0
        || options.rows == 0
        || options.cell_width == 0
        || options.cell_height == 0
        || options.font_size == 0
    {
        bail!("capture dimensions must be greater than zero");
    }

    let backend = TestBackend::new(options.columns, options.rows);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| draw(frame, app))?;
    let buffer = terminal.backend().buffer();

    let pixel_width = u32::from(options.columns) * u32::from(options.cell_width);
    let pixel_height = u32::from(options.rows) * u32::from(options.cell_height);
    let baseline = options
        .cell_height
        .saturating_sub((options.cell_height.saturating_sub(options.font_size)) / 2 + 1);
    let background = color_hex(theme().background, "#000000");
    let mut svg =
        String::with_capacity(usize::from(options.columns) * usize::from(options.rows) * 24);

    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pixel_width}\" height=\"{pixel_height}\" viewBox=\"0 0 {pixel_width} {pixel_height}\">\n"
    ));
    svg.push_str(&format!(
        "<rect width=\"100%\" height=\"100%\" fill=\"{background}\"/>\n"
    ));
    svg.push_str(&format!(
        "<g font-family=\"Cascadia Mono, Cascadia Code, Consolas, DejaVu Sans Mono, monospace\" font-size=\"{}\" text-anchor=\"middle\">\n",
        options.font_size
    ));

    // Paint non-canvas cell backgrounds first so selected rows remain solid.
    for y in 0..options.rows {
        for x in 0..options.columns {
            let cell = &buffer[(x, y)];
            let (_, bg) = resolved_colors(cell);
            let bg = color_hex(bg, &background);
            if bg != background {
                svg.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
                    u32::from(x) * u32::from(options.cell_width),
                    u32::from(y) * u32::from(options.cell_height),
                    options.cell_width,
                    options.cell_height,
                    bg
                ));
            }
        }
    }

    for y in 0..options.rows {
        for x in 0..options.columns {
            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();
            if symbol.trim().is_empty() || cell.modifier.contains(Modifier::HIDDEN) {
                continue;
            }
            let (fg, _) = resolved_colors(cell);
            let fg = color_hex(fg, "#f5f5f5");
            let font_weight = if cell.modifier.contains(Modifier::BOLD) {
                "700"
            } else {
                "400"
            };
            let opacity = if cell.modifier.contains(Modifier::DIM) {
                "0.65"
            } else {
                "1"
            };
            let px =
                u32::from(x) * u32::from(options.cell_width) + u32::from(options.cell_width) / 2;
            let py = u32::from(y) * u32::from(options.cell_height) + u32::from(baseline);
            svg.push_str(&format!(
                "<text x=\"{px}\" y=\"{py}\" fill=\"{fg}\" fill-opacity=\"{opacity}\" font-weight=\"{font_weight}\">{}</text>\n",
                escape_xml(symbol)
            ));
        }
    }

    svg.push_str("</g>\n</svg>\n");
    Ok(svg)
}

fn resolved_colors(cell: &Cell) -> (Color, Color) {
    if cell.modifier.contains(Modifier::REVERSED) {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    }
}

fn color_hex(color: Color, reset: &str) -> String {
    let (red, green, blue) = match color {
        Color::Reset => return reset.to_owned(),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (102, 102, 102),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::White => (255, 255, 255),
        Color::Rgb(red, green, blue) => (red, green, blue),
        Color::Indexed(index) => indexed_rgb(index),
    };
    format!("#{red:02x}{green:02x}{blue:02x}")
}

fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let value = index - 16;
            let component = |part: u8| if part == 0 { 0 } else { 55 + part * 40 };
            (
                component(value / 36),
                component((value % 36) / 6),
                component(value % 6),
            )
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoApi;
    use std::sync::Arc;

    #[test]
    fn capture_contains_the_rendered_landing_page() {
        let mut app = App::new(Arc::new(DemoApi::new()), true);
        let svg = capture_svg(&mut app, CaptureOptions::landing()).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("█"));
        assert!(svg.contains(">S</text>"));
        assert!(svg.contains("font-weight=\"700\""));
        assert!(svg.ends_with("</svg>\n"));
    }
}
