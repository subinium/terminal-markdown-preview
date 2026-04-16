//! Cat-mode: render markdown to stdout with ANSI styling + inline images.
//! Used on non-Kitty terminals for pixel-perfect images with native scrolling.

use std::io::Write;

use crate::markdown::{self, Inline};
use crate::render::{self, Block, RenderedImage};

// ANSI escape helpers — use 24bit RGB for max terminal compatibility
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const UNDERLINE: &str = "\x1b[4m";
const STRIKE: &str = "\x1b[9m";

fn fg(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

fn bg(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[48;2;{r};{g};{b}m")
}

/// Render markdown to stdout (cat mode)
pub fn render_to_stdout(content: &str) {
    let mut blocks = markdown::parse(content);

    // Render mermaid synchronously (cat mode is one-shot)
    for block in &mut blocks {
        if let Block::Mermaid { source, image } = block {
            *image = render::render_mermaid_sync(source);
        }
    }

    let out = std::io::stdout();
    let mut w = std::io::BufWriter::new(out.lock());

    let (term_cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
    let width = (term_cols as usize).saturating_sub(4);

    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            let _ = writeln!(w);
        }
        render_block(&mut w, block, width);
    }
    let _ = w.flush();
}

fn render_block(w: &mut impl Write, block: &Block, width: usize) {
    match block {
        Block::Heading { level, text } => render_heading(w, *level, text, width),
        Block::Paragraph { inlines } => render_paragraph(w, inlines),
        Block::CodeBlock { lang, code } => render_code_block(w, lang.as_deref(), code),
        Block::Mermaid { source: _, image } => render_mermaid(w, image),
        Block::Math { source, display } => render_math(w, source, *display),
        Block::List { items } => render_list(w, items),
        Block::BlockQuote { text } => render_blockquote(w, text),
        Block::ThematicBreak => render_hr(w, width),
        Block::Table { headers, rows } => render_table(w, headers, rows),
    }
}

fn render_heading(w: &mut impl Write, level: u8, text: &str, width: usize) {
    let prefix = "#".repeat(level as usize);
    let dim = fg(140, 140, 140);
    match level {
        1 => {
            let c = fg(130, 170, 255);
            let _ = writeln!(w);
            let _ = writeln!(w, "{dim}{prefix}{RESET} {BOLD}{c}{text}{RESET}");
            let _ = writeln!(w, "{dim}{}{RESET}", "━".repeat(width));
            let _ = writeln!(w);
        }
        2 => {
            let c = fg(130, 200, 130);
            let _ = writeln!(w);
            let _ = writeln!(w, "{dim}{prefix}{RESET} {BOLD}{c}{text}{RESET}");
            let _ = writeln!(w, "{dim}{}{RESET}", "─".repeat(width * 2 / 3));
        }
        3 => {
            let c = fg(220, 200, 100);
            let _ = writeln!(w);
            let _ = writeln!(w, "{dim}{prefix}{RESET} {BOLD}{c}{text}{RESET}");
        }
        4 => {
            let _ = writeln!(w, "{dim}{prefix}{RESET} {BOLD}{text}{RESET}");
        }
        _ => {
            let _ = writeln!(w, "{dim}{prefix} {text}{RESET}");
        }
    }
}

fn render_paragraph(w: &mut impl Write, inlines: &[Inline]) {
    let blue = fg(130, 170, 255);
    let code_fg = fg(230, 150, 150);
    let code_bg = bg(30, 30, 30);
    let math_fg = fg(190, 160, 250);

    for inline in inlines {
        match inline {
            Inline::Text(t) => {
                let _ = write!(w, "{t}");
            }
            Inline::Bold(t) => {
                let _ = write!(w, "{BOLD}{t}{RESET}");
            }
            Inline::Italic(t) => {
                let _ = write!(w, "{ITALIC}{t}{RESET}");
            }
            Inline::Strikethrough(t) => {
                let _ = write!(w, "{STRIKE}{DIM}{t}{RESET}");
            }
            Inline::Code(t) => {
                let _ = write!(w, "{code_bg}{code_fg} {t} {RESET}");
            }
            Inline::Link { text, url } => {
                // OSC 8 hyperlink
                let _ = write!(
                    w,
                    "\x1b]8;;{url}\x1b\\{UNDERLINE}{blue}{text}{RESET}\x1b]8;;\x1b\\"
                );
            }
            Inline::Math { source } => {
                let rendered = render::unicode_math_pub(source);
                let _ = write!(w, "{math_fg}{rendered}{RESET}");
            }
            Inline::SoftBreak | Inline::LineBreak => {
                let _ = write!(w, " ");
            }
        }
    }
    let _ = writeln!(w);
}

// ── Code Block with syntax highlighting ──────────────────────────────────

fn render_code_block(w: &mut impl Write, lang: Option<&str>, code: &str) {
    let code_bg = bg(35, 35, 42);
    let plain_fg = fg(200, 200, 200);
    let (term_cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
    let bw = (term_cols as usize).saturating_sub(2); // box width

    let _ = writeln!(w);
    if let Some(lang) = lang {
        let _ = writeln!(w, "{DIM}{ITALIC}{lang}{RESET}");
    }

    let trimmed = code.trim_end();

    // Top padding
    let _ = writeln!(w, "{code_bg}{:bw$}{RESET}", "");

    // Try tree-sitter syntax highlighting via SLT
    if let Some(lang) = lang {
        let theme = slt::Theme::dark();
        if let Some(highlighted_lines) = slt::syntax::highlight_code(trimmed, lang, &theme) {
            for line_spans in &highlighted_lines {
                let _ = write!(w, "{code_bg}    ");
                let mut len = 4usize;
                for (text, style) in line_spans {
                    let t = text.trim_end_matches('\n');
                    len += t.len();
                    if let Some(color) = style.fg {
                        let (r, g, b) = color_to_rgb(color);
                        let _ = write!(w, "{}{t}", fg(r, g, b));
                    } else {
                        let _ = write!(w, "{plain_fg}{t}");
                    }
                }
                // Pad remaining width to fill the box
                let pad = bw.saturating_sub(len);
                let _ = writeln!(w, "{:pad$}{RESET}", "");
            }
            let _ = writeln!(w, "{code_bg}{:bw$}{RESET}", ""); // bottom padding
            let _ = writeln!(w);
            return;
        }
    }

    // Fallback: plain text
    for line in trimmed.lines() {
        let content = format!("    {line}");
        let pad = bw.saturating_sub(content.len());
        let _ = writeln!(w, "{code_bg}{plain_fg}{content}{:pad$}{RESET}", "");
    }
    let _ = writeln!(w, "{code_bg}{:bw$}{RESET}", ""); // bottom padding
    let _ = writeln!(w);
}

/// Extract RGB from SLT Color
fn color_to_rgb(c: slt::Color) -> (u8, u8, u8) {
    match c {
        slt::Color::Rgb(r, g, b) => (r, g, b),
        slt::Color::Indexed(n) => indexed_to_rgb(n),
        slt::Color::Red => (255, 80, 80),
        slt::Color::Green => (80, 255, 80),
        slt::Color::Blue => (80, 80, 255),
        slt::Color::Yellow => (255, 255, 80),
        slt::Color::Cyan => (80, 255, 255),
        slt::Color::Magenta => (255, 80, 255),
        slt::Color::White => (220, 220, 220),
        slt::Color::Black => (30, 30, 30),
        _ => (200, 200, 200),
    }
}

/// Convert 256-color index to approximate RGB
fn indexed_to_rgb(n: u8) -> (u8, u8, u8) {
    match n {
        0 => (0, 0, 0),
        1 => (170, 0, 0),
        2 => (0, 170, 0),
        3 => (170, 85, 0),
        4 => (0, 0, 170),
        5 => (170, 0, 170),
        6 => (0, 170, 170),
        7 => (170, 170, 170),
        8 => (85, 85, 85),
        9 => (255, 85, 85),
        10 => (85, 255, 85),
        11 => (255, 255, 85),
        12 => (85, 85, 255),
        13 => (255, 85, 255),
        14 => (85, 255, 255),
        15 => (255, 255, 255),
        16..=231 => {
            let n = n - 16;
            let r = (n / 36) * 51;
            let g = ((n % 36) / 6) * 51;
            let b = (n % 6) * 51;
            (r, g, b)
        }
        232..=255 => {
            let v = 8 + (n - 232) * 10;
            (v, v, v)
        }
    }
}

// ── Mermaid ──────────────────────────────────────────────────────────────

fn render_mermaid(w: &mut impl Write, image: &Option<RenderedImage>) {
    if let Some(img) = image {
        let png = render::rgba_to_png_pub(&img.rgba, img.width, img.height);
        if let Some(png) = png {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
            let _ = write!(
                w,
                "\x1b]1337;File=inline=1;width=auto;preserveAspectRatio=1:{b64}\x07"
            );
            let _ = writeln!(w);
        }
    } else {
        let dim = fg(140, 140, 140);
        let _ = writeln!(w, "{dim}  (mermaid render failed){RESET}");
    }
}

// ── Math ─────────────────────────────────────────────────────────────────

fn render_math(w: &mut impl Write, source: &str, display: bool) {
    let math_fg = fg(190, 160, 250);
    let rendered = render::unicode_math_pub(source);
    if display {
        let _ = writeln!(w);
        let _ = writeln!(w, "    {math_fg}{rendered}{RESET}");
        let _ = writeln!(w);
    } else {
        let _ = write!(w, "{math_fg}{rendered}{RESET}");
    }
}

// ── List ─────────────────────────────────────────────────────────────────

fn render_list(w: &mut impl Write, items: &[String]) {
    let dim = fg(140, 140, 140);
    for item in items {
        let _ = writeln!(w, "  {dim}•{RESET}  {item}");
    }
}

// ── Blockquote ───────────────────────────────────────────────────────────

fn render_blockquote(w: &mut impl Write, text: &str) {
    let code_bg = bg(20, 20, 20);
    let dim = fg(140, 140, 140);
    let _ = writeln!(
        w,
        "{code_bg}  {dim}│{RESET}{code_bg}  {ITALIC}{DIM}{text}{RESET}"
    );
}

// ── Horizontal Rule ──────────────────────────────────────────────────────

fn render_hr(w: &mut impl Write, width: usize) {
    let dim = fg(140, 140, 140);
    let _ = writeln!(w);
    let _ = writeln!(w, "{dim}{}{RESET}", "─".repeat(width));
    let _ = writeln!(w);
}

// ── Table ────────────────────────────────────────────────────────────────

fn render_table(w: &mut impl Write, headers: &[String], rows: &[Vec<String>]) {
    if headers.is_empty() {
        return;
    }

    let blue = fg(130, 170, 255);
    let dim = fg(140, 140, 140);
    let num_cols = headers.len();
    let mut col_widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }
    }
    for cw in &mut col_widths {
        *cw += 2;
    }

    for (i, header) in headers.iter().enumerate() {
        let cw = col_widths.get(i).copied().unwrap_or(10);
        let _ = write!(w, "{BOLD}{blue}{:<cw$}{RESET}", header, cw = cw);
    }
    let _ = writeln!(w);

    for &cw in &col_widths {
        let _ = write!(w, "{dim}{:─<cw$}{RESET}", "", cw = cw);
    }
    let _ = writeln!(w);

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let cw = col_widths.get(i).copied().unwrap_or(10);
            let _ = write!(w, "{:<cw$}", cell, cw = cw);
        }
        let _ = writeln!(w);
    }
}
