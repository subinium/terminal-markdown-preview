//! Cat-mode: render markdown to stdout with ANSI styling + inline images.
//! Used on non-Kitty terminals for pixel-perfect images with native scrolling.

use std::io::Write;

use crate::markdown::{self, Inline};
use crate::render::{self, Block, RenderedImage};

// ANSI color codes
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const UNDERLINE: &str = "\x1b[4m";
const STRIKE: &str = "\x1b[9m";
const FG_BLUE: &str = "\x1b[38;5;111m";
const FG_GREEN: &str = "\x1b[38;5;114m";
const FG_YELLOW: &str = "\x1b[38;5;222m";
const FG_CODE: &str = "\x1b[38;5;210m";
const FG_MATH: &str = "\x1b[38;5;183m";
const FG_DIM: &str = "\x1b[38;5;245m";
const BG_CODE: &str = "\x1b[48;5;234m";

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
    match level {
        1 => {
            let _ = writeln!(w);
            let _ = writeln!(w, "{FG_DIM}{prefix}{RESET} {BOLD}{FG_BLUE}{text}{RESET}");
            let _ = writeln!(w, "{FG_DIM}{}{RESET}", "━".repeat(width));
            let _ = writeln!(w);
        }
        2 => {
            let _ = writeln!(w);
            let _ = writeln!(w, "{FG_DIM}{prefix}{RESET} {BOLD}{FG_GREEN}{text}{RESET}");
            let _ = writeln!(w, "{FG_DIM}{}{RESET}", "─".repeat(width * 2 / 3));
        }
        3 => {
            let _ = writeln!(w);
            let _ = writeln!(w, "{FG_DIM}{prefix}{RESET} {BOLD}{FG_YELLOW}{text}{RESET}");
        }
        4 => {
            let _ = writeln!(w, "{FG_DIM}{prefix}{RESET} {BOLD}{text}{RESET}");
        }
        _ => {
            let _ = writeln!(w, "{FG_DIM}{prefix} {text}{RESET}");
        }
    }
}

fn render_paragraph(w: &mut impl Write, inlines: &[Inline]) {
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
                let _ = write!(w, "{BG_CODE}{FG_CODE} {t} {RESET}");
            }
            Inline::Link { text, url } => {
                // OSC 8 hyperlink
                let _ = write!(
                    w,
                    "\x1b]8;;{url}\x1b\\{UNDERLINE}{FG_BLUE}{text}{RESET}\x1b]8;;\x1b\\"
                );
            }
            Inline::Math { source } => {
                let rendered = render::unicode_math_pub(source);
                let _ = write!(w, "{FG_MATH}{rendered}{RESET}");
            }
            Inline::SoftBreak | Inline::LineBreak => {
                let _ = write!(w, " ");
            }
        }
    }
    let _ = writeln!(w);
}

fn render_code_block(w: &mut impl Write, lang: Option<&str>, code: &str) {
    let _ = writeln!(w);
    if let Some(lang) = lang {
        let _ = writeln!(w, "{DIM}{ITALIC}{lang}{RESET}");
    }
    for line in code.trim_end().lines() {
        let _ = writeln!(w, "{BG_CODE}    {FG_CODE}{line}{RESET}");
    }
    let _ = writeln!(w);
}

fn render_mermaid(w: &mut impl Write, image: &Option<RenderedImage>) {
    if let Some(img) = image {
        // iTerm2 OSC 1337 inline image — pixel-perfect, scrolls natively
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
        let _ = writeln!(w, "{DIM}  (mermaid render failed){RESET}");
    }
}

fn render_math(w: &mut impl Write, source: &str, display: bool) {
    let rendered = render::unicode_math_pub(source);
    if display {
        let _ = writeln!(w);
        let _ = writeln!(w, "    {FG_MATH}{rendered}{RESET}");
        let _ = writeln!(w);
    } else {
        let _ = write!(w, "{FG_MATH}{rendered}{RESET}");
    }
}

fn render_list(w: &mut impl Write, items: &[String]) {
    for item in items {
        let _ = writeln!(w, "  {FG_DIM}•{RESET}  {item}");
    }
}

fn render_blockquote(w: &mut impl Write, text: &str) {
    let _ = writeln!(
        w,
        "{BG_CODE}  {FG_DIM}│{RESET}{BG_CODE}  {ITALIC}{DIM}{text}{RESET}"
    );
}

fn render_hr(w: &mut impl Write, width: usize) {
    let _ = writeln!(w);
    let _ = writeln!(w, "{FG_DIM}{}{RESET}", "─".repeat(width));
    let _ = writeln!(w);
}

fn render_table(w: &mut impl Write, headers: &[String], rows: &[Vec<String>]) {
    if headers.is_empty() {
        return;
    }

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

    // Header
    for (i, header) in headers.iter().enumerate() {
        let cw = col_widths.get(i).copied().unwrap_or(10);
        let _ = write!(w, "{BOLD}{FG_BLUE}{:<cw$}{RESET}", header, cw = cw);
    }
    let _ = writeln!(w);

    // Separator
    for &cw in &col_widths {
        let _ = write!(w, "{FG_DIM}{:─<cw$}{RESET}", "", cw = cw);
    }
    let _ = writeln!(w);

    // Rows
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let cw = col_widths.get(i).copied().unwrap_or(10);
            let _ = write!(w, "{:<cw$}", cell, cw = cw);
        }
        let _ = writeln!(w);
    }
}
