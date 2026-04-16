use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use slt::*;

use crate::markdown::{self, Inline};

/// Check if terminal supports Kitty graphics protocol
fn supports_kitty() -> bool {
    if let Ok(term) = std::env::var("TERM") {
        if term.contains("kitty") || term.contains("xterm-kitty") {
            return true;
        }
    }
    if std::env::var("KITTY_WINDOW_ID").is_ok() {
        return true;
    }
    if let Ok(prog) = std::env::var("TERM_PROGRAM") {
        let p = prog.to_lowercase();
        if p.contains("wezterm") || p.contains("ghostty") {
            return true;
        }
    }
    false
}

static USE_KITTY: LazyLock<bool> = LazyLock::new(supports_kitty);

// ── Caches — persist across file reloads ─────────────────────────────────
static MERMAID_CACHE: LazyLock<Mutex<HashMap<u64, RenderedImage>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Pre-rendered image data (RGBA pixels)
#[derive(Debug, Clone)]
pub struct RenderedImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph {
        inlines: Vec<Inline>,
    },
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
    Mermaid {
        source: String,
        image: Option<RenderedImage>,
    },
    Math {
        source: String,
        display: bool,
    },
    List {
        items: Vec<String>,
    },
    BlockQuote {
        text: String,
    },
    ThematicBreak,
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

// ── Colors — fg only, no bg anywhere ─────────────────────────────────────
const DIM: Color = Color::Indexed(245);
const ACCENT: Color = Color::Indexed(111);
const H2_CLR: Color = Color::Indexed(114);
const H3_CLR: Color = Color::Indexed(222);
const CODE_CLR: Color = Color::Indexed(210);
const MATH_CLR: Color = Color::Indexed(183);
const LINK_CLR: Color = Color::Indexed(111);

// ── Font database (cached, loaded once) ──────────────────────────────────

static FONTDB: LazyLock<Arc<resvg::usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = resvg::usvg::fontdb::Database::new();
    db.load_system_fonts();
    db.set_sans_serif_family("Helvetica");
    Arc::new(db)
});

const MAX_IMG_W: u32 = 1600;
const MAX_IMG_H: u32 = 1200;

/// Parse markdown content into blocks (mermaid images NOT rendered yet)
pub fn render_markdown_fast(content: &str) -> Vec<Block> {
    markdown::parse(content)
}

/// Render all pending mermaid blocks in a background thread.
/// Returns a receiver that yields (block_index, rendered_image) pairs.
pub fn render_mermaid_async(blocks: &[Block]) -> std::sync::mpsc::Receiver<(usize, RenderedImage)> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Collect mermaid sources with their indices
    let mermaid_jobs: Vec<(usize, String)> = blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            if let Block::Mermaid {
                source,
                image: None,
            } = b
            {
                Some((i, source.clone()))
            } else {
                None
            }
        })
        .collect();

    if !mermaid_jobs.is_empty() {
        // Warm up font database before spawning threads
        let _ = &*FONTDB;

        std::thread::spawn(move || {
            // Render all diagrams in parallel
            std::thread::scope(|s| {
                for (idx, source) in mermaid_jobs {
                    let tx = tx.clone();
                    s.spawn(move || {
                        if let Some(img) = render_mermaid_to_rgba(&source) {
                            let _ = tx.send((idx, img));
                        }
                    });
                }
            });
        });
    }

    rx
}

/// Render a single mermaid diagram synchronously (for cat mode)
pub fn render_mermaid_sync(source: &str) -> Option<RenderedImage> {
    render_mermaid_to_rgba(source)
}

/// Public unicode math for cat mode
pub fn unicode_math_pub(source: &str) -> String {
    unicode_math(source)
}

/// Public RGBA→PNG for cat mode
pub fn rgba_to_png_pub(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let mut buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut buf);
    encoder
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(buf)
}

/// Hash a mermaid source string for cache lookup
fn hash_source(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Render mermaid source to RGBA pixel data (with cache)
fn render_mermaid_to_rgba(source: &str) -> Option<RenderedImage> {
    let key = hash_source(source);

    // Check cache first
    if let Ok(cache) = MERMAID_CACHE.lock()
        && let Some(cached) = cache.get(&key)
    {
        return Some(cached.clone());
    }

    let opts = mermaid_rs_renderer::RenderOptions::modern()
        .with_node_spacing(80.0)
        .with_rank_spacing(60.0);
    let svg_str = mermaid_rs_renderer::render_with_options(source, opts).ok()?;

    let options = resvg::usvg::Options {
        fontdb: FONTDB.clone(),
        ..Default::default()
    };

    let tree = resvg::usvg::Tree::from_str(&svg_str, &options).ok()?;
    let size = tree.size();

    if size.width() < 1.0 || size.height() < 1.0 {
        return None;
    }

    let scale_x = MAX_IMG_W as f32 / size.width();
    let scale_y = MAX_IMG_H as f32 / size.height();
    let scale = scale_x.min(scale_y).clamp(1.0, 3.0);

    // Ensure even dimensions (some terminals/protocols handle odd sizes poorly)
    let w = (((size.width() * scale) as u32).max(16)) & !1;
    let h = (((size.height() * scale) as u32).max(16)) & !1;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);

    // Center the diagram in the pixmap
    let actual_w = size.width() * scale;
    let actual_h = size.height() * scale;
    let offset_x = (w as f32 - actual_w) / 2.0;
    let offset_y = (h as f32 - actual_h) / 2.0;
    let transform =
        resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(offset_x, offset_y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let rgba = pixmap.data().to_vec();
    let expected = (w as usize) * (h as usize) * 4;
    if rgba.len() != expected {
        return None;
    }

    let img = RenderedImage {
        rgba,
        width: w,
        height: h,
    };

    // Store in cache for live reload
    if let Ok(mut cache) = MERMAID_CACHE.lock() {
        cache.insert(key, img.clone());
    }

    Some(img)
}

/// Get usable content width (terminal width minus padding)
fn content_width(_ui: &Context) -> usize {
    let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
    (cols as usize).saturating_sub(10) // px(3) each side + scrollbar margin
}

/// Render all blocks
pub fn render_blocks(ui: &mut Context, blocks: &[Block]) {
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            ui.text("");
        }
        render_block(ui, block);
    }
}

fn render_block(ui: &mut Context, block: &Block) {
    match block {
        Block::Heading { level, text } => render_heading(ui, *level, text),
        Block::Paragraph { inlines } => render_paragraph(ui, inlines),
        Block::CodeBlock { lang, code } => render_code_block(ui, lang.as_deref(), code),
        Block::Mermaid { source, image } => render_mermaid(ui, source, image),
        Block::Math { source, display } => render_math(ui, source, *display),
        Block::List { items } => render_list(ui, items),
        Block::BlockQuote { text } => render_blockquote(ui, text),
        Block::ThematicBreak => render_hr(ui),
        Block::Table { headers, rows } => render_table(ui, headers, rows),
    }
}

// ── Headings ─────────────────────────────────────────────────────────────

fn render_heading(ui: &mut Context, level: u8, text: &str) {
    let w = content_width(ui);
    let prefix = "#".repeat(level as usize);
    match level {
        1 => {
            ui.text("");
            ui.line(|ui| {
                ui.text(format!("{prefix} ")).fg(DIM);
                ui.text(text).bold().fg(ACCENT);
            });
            ui.text("━".repeat(w)).fg(DIM);
            ui.text("");
        }
        2 => {
            ui.text("");
            ui.line(|ui| {
                ui.text(format!("{prefix} ")).fg(DIM);
                ui.text(text).bold().fg(H2_CLR);
            });
            ui.text("─".repeat(w * 2 / 3)).fg(DIM);
        }
        3 => {
            ui.text("");
            ui.line(|ui| {
                ui.text(format!("{prefix} ")).fg(DIM);
                ui.text(text).bold().fg(H3_CLR);
            });
        }
        4 => {
            ui.line(|ui| {
                ui.text(format!("{prefix} ")).fg(DIM);
                ui.text(text).bold();
            });
        }
        _ => {
            ui.line(|ui| {
                ui.text(format!("{prefix} ")).fg(DIM);
                ui.text(text).dim();
            });
        }
    }
}

// ── Paragraph ────────────────────────────────────────────────────────────

fn render_paragraph(ui: &mut Context, inlines: &[Inline]) {
    if inlines.is_empty() {
        return;
    }
    ui.line_wrap(|ui| {
        for inline in inlines {
            match inline {
                Inline::Text(t) => {
                    ui.text(t);
                }
                Inline::Bold(t) => {
                    ui.text(t).bold();
                }
                Inline::Italic(t) => {
                    ui.text(t).italic();
                }
                Inline::Strikethrough(t) => {
                    ui.text(t).strikethrough().dim();
                }
                Inline::Code(t) => {
                    ui.text(format!(" {t} "))
                        .fg(CODE_CLR)
                        .bg(Color::Indexed(234));
                }
                Inline::Link { text, url } => {
                    ui.link(text, url).fg(LINK_CLR).underline();
                }
                Inline::Math { source } => {
                    ui.text(unicode_math(source)).fg(MATH_CLR);
                }
                Inline::SoftBreak | Inline::LineBreak => {
                    ui.text(" ");
                }
            }
        }
    });
}

// ── Code Block — tree-sitter syntax highlighting ─────────────────────────

fn render_code_block(ui: &mut Context, lang: Option<&str>, code: &str) {
    ui.text("");

    if let Some(lang) = lang {
        ui.text(lang).dim().italic();
    }

    let trimmed = code.trim_end();

    // Subtle bg — just enough to distinguish code from prose
    let code_bg = Color::Indexed(234); // #1c1c1c — barely lighter than pure black

    if let Some(lang) = lang {
        let theme = *ui.theme();
        if let Some(highlighted_lines) = slt::syntax::highlight_code(trimmed, lang, &theme) {
            let _ = ui.container().bg(code_bg).px(4).py(1).col(|ui| {
                for line_spans in &highlighted_lines {
                    if line_spans.is_empty() {
                        ui.text("");
                    } else {
                        ui.line(|ui| {
                            for (text, style) in line_spans {
                                ui.styled(text, *style);
                            }
                        });
                    }
                }
            });
            ui.text("");
            return;
        }
    }

    let _ = ui.container().bg(code_bg).px(4).py(1).col(|ui| {
        for line in trimmed.lines() {
            ui.text(line).fg(CODE_CLR);
        }
    });

    ui.text("");
}

// ── Mermaid — multi-protocol image rendering ─────────────────────────────

/// Calculate display dimensions for a mermaid image
fn mermaid_display_size(ui: &Context, img: &RenderedImage) -> u32 {
    let w = content_width(ui) as u32;
    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let max_rows = (term_rows as u32) * 2 / 3;

    let aspect = img.width as f32 / img.height as f32;
    let mut cols = if aspect > 1.5 {
        (w * 9 / 10).min(img.width / 6)
    } else {
        (w * 2 / 3).min(img.width / 6)
    };
    cols = cols.max(30).min(term_cols as u32 - 8);

    let est_rows = (cols as f32 / aspect / 2.0) as u32;
    if est_rows > max_rows {
        cols = (max_rows as f32 * aspect * 2.0) as u32;
        cols = cols.max(20);
    }
    cols
}

fn render_mermaid(ui: &mut Context, source: &str, image: &Option<RenderedImage>) {
    match image {
        Some(img) => {
            let cols = mermaid_display_size(ui, img);
            let aspect = img.width as f32 / img.height as f32;

            if *USE_KITTY {
                // Pixel-perfect: Kitty protocol (cached by SLT's KittyImageManager)
                let _ = ui.kitty_image_fit(&img.rgba, img.width, img.height, cols);
            } else {
                // HalfBlock fallback: cell-based, smooth scroll on all terminals
                let dyn_img = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone())
                    .map(image::DynamicImage::ImageRgba8);
                if let Some(dyn_img) = dyn_img {
                    let rows = (cols as f32 / aspect / 2.0) as u32;
                    let half = HalfBlockImage::from_dynamic(&dyn_img, cols, rows.max(4));
                    let _ = ui.image(&half);
                }
            }
        }
        None => {
            if source.is_empty() {
                ui.text("  mermaid (empty)").dim().italic();
            } else {
                ui.text("  ⏳ rendering mermaid...").dim().italic();
            }
        }
    }
}

// ── Math ─────────────────────────────────────────────────────────────────

fn render_math(ui: &mut Context, source: &str, display: bool) {
    let rendered = unicode_math(source);
    if display {
        ui.text("");
        ui.text(&rendered).fg(MATH_CLR).align(Align::Center);
        ui.text("");
    } else {
        ui.text(&rendered).fg(MATH_CLR);
    }
}

fn unicode_math(source: &str) -> String {
    let mut r = source.to_string();

    r = r.replace("\\frac{1}{2}", "½");
    r = r.replace("\\frac{1}{3}", "⅓");
    r = r.replace("\\frac{1}{4}", "¼");
    r = r.replace("\\frac{3}{4}", "¾");

    for (cmd, sym) in [
        ("\\alpha", "α"),
        ("\\beta", "β"),
        ("\\gamma", "γ"),
        ("\\delta", "δ"),
        ("\\epsilon", "ε"),
        ("\\zeta", "ζ"),
        ("\\eta", "η"),
        ("\\theta", "θ"),
        ("\\iota", "ι"),
        ("\\kappa", "κ"),
        ("\\lambda", "λ"),
        ("\\mu", "μ"),
        ("\\nu", "ν"),
        ("\\xi", "ξ"),
        ("\\pi", "π"),
        ("\\rho", "ρ"),
        ("\\sigma", "σ"),
        ("\\tau", "τ"),
        ("\\phi", "φ"),
        ("\\chi", "χ"),
        ("\\psi", "ψ"),
        ("\\omega", "ω"),
        ("\\Sigma", "Σ"),
        ("\\Pi", "Π"),
        ("\\Omega", "Ω"),
        ("\\Delta", "Δ"),
        ("\\Gamma", "Γ"),
        ("\\Lambda", "Λ"),
        ("\\Phi", "Φ"),
        ("\\Psi", "Ψ"),
        ("\\Theta", "Θ"),
    ] {
        r = r.replace(cmd, sym);
    }

    for (cmd, sym) in [
        ("\\times", "×"),
        ("\\div", "÷"),
        ("\\pm", "±"),
        ("\\mp", "∓"),
        ("\\cdot", "·"),
        ("\\cdots", "⋯"),
        ("\\ldots", "…"),
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("\\neq", "≠"),
        ("\\approx", "≈"),
        ("\\equiv", "≡"),
        ("\\infty", "∞"),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\int", "∫"),
        ("\\partial", "∂"),
        ("\\nabla", "∇"),
        ("\\forall", "∀"),
        ("\\exists", "∃"),
        ("\\in", "∈"),
        ("\\notin", "∉"),
        ("\\subset", "⊂"),
        ("\\supset", "⊃"),
        ("\\subseteq", "⊆"),
        ("\\supseteq", "⊇"),
        ("\\cup", "∪"),
        ("\\cap", "∩"),
        ("\\emptyset", "∅"),
        ("\\to", "→"),
        ("\\rightarrow", "→"),
        ("\\leftarrow", "←"),
        ("\\Rightarrow", "⇒"),
        ("\\Leftarrow", "⇐"),
        ("\\leftrightarrow", "↔"),
        ("\\Leftrightarrow", "⇔"),
        ("\\sqrt", "√"),
        ("\\langle", "⟨"),
        ("\\rangle", "⟩"),
        ("\\lceil", "⌈"),
        ("\\rceil", "⌉"),
        ("\\lfloor", "⌊"),
        ("\\rfloor", "⌋"),
        ("\\cos", "cos"),
        ("\\sin", "sin"),
        ("\\tan", "tan"),
        ("\\log", "log"),
        ("\\ln", "ln"),
        ("\\lim", "lim"),
        ("\\max", "max"),
        ("\\min", "min"),
    ] {
        r = r.replace(cmd, sym);
    }

    for (cmd, sym) in [
        ("^{0}", "⁰"),
        ("^{1}", "¹"),
        ("^{2}", "²"),
        ("^{3}", "³"),
        ("^{4}", "⁴"),
        ("^{5}", "⁵"),
        ("^{6}", "⁶"),
        ("^{7}", "⁷"),
        ("^{8}", "⁸"),
        ("^{9}", "⁹"),
        ("^{n}", "ⁿ"),
        ("^{i}", "ⁱ"),
        ("^0", "⁰"),
        ("^1", "¹"),
        ("^2", "²"),
        ("^3", "³"),
        ("^n", "ⁿ"),
    ] {
        r = r.replace(cmd, sym);
    }

    for (cmd, sym) in [
        ("_{0}", "₀"),
        ("_{1}", "₁"),
        ("_{2}", "₂"),
        ("_{3}", "₃"),
        ("_{4}", "₄"),
        ("_{5}", "₅"),
        ("_{6}", "₆"),
        ("_{7}", "₇"),
        ("_{8}", "₈"),
        ("_{9}", "₉"),
        ("_{i}", "ᵢ"),
        ("_{n}", "ₙ"),
    ] {
        r = r.replace(cmd, sym);
    }

    while let Some(start) = r.find("\\frac{") {
        if let Some(end1) = r[start + 6..].find('}') {
            let num = r[start + 6..start + 6 + end1].to_string();
            let rest = &r[start + 6 + end1 + 1..];
            if let Some(stripped) = rest.strip_prefix('{')
                && let Some(end2) = stripped.find('}')
            {
                let den = stripped[..end2].to_string();
                let after = stripped[end2 + 1..].to_string();
                r = format!("{}({})/({}){}", &r[..start], num, den, after);
                continue;
            }
        }
        break;
    }

    r = r.replace("\\left", "");
    r = r.replace("\\right", "");
    r = r.replace("\\,", " ");
    r = r.replace("\\;", " ");
    r = r.replace("\\quad", "  ");
    r = r.replace("\\text{", "");
    r = r.replace("\\mathbf{", "");
    r = r.replace(['{', '}'], "");
    r
}

// ── List ─────────────────────────────────────────────────────────────────

fn render_list(ui: &mut Context, items: &[String]) {
    for item in items {
        ui.line(|ui| {
            ui.text("  • ").fg(DIM);
            ui.text(item);
        });
    }
}

// ── Blockquote ───────────────────────────────────────────────────────────

fn render_blockquote(ui: &mut Context, text: &str) {
    let _ = ui
        .container()
        .bg(Color::Indexed(234))
        .border(Border::Single)
        .border_sides(BorderSides::vertical())
        .border_top(false)
        .border_bottom(false)
        .border_fg(DIM)
        .px(2)
        .py(1)
        .col(|ui| {
            ui.text(text).italic().dim();
        });
}

// ── Horizontal Rule ──────────────────────────────────────────────────────

fn render_hr(ui: &mut Context) {
    let w = content_width(ui);
    ui.text("");
    ui.text("─".repeat(w)).fg(DIM);
    ui.text("");
}

// ── Table ────────────────────────────────────────────────────────────────

fn render_table(ui: &mut Context, headers: &[String], rows: &[Vec<String>]) {
    if headers.is_empty() {
        return;
    }

    // Calculate column widths
    let num_cols = headers.len();
    let mut col_widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }
    }
    // Add padding
    for w in &mut col_widths {
        *w += 2;
    }

    // Header
    ui.line(|ui| {
        for (i, header) in headers.iter().enumerate() {
            let w = col_widths.get(i).copied().unwrap_or(10);
            ui.text(format!("{:<w$}", header, w = w)).bold().fg(ACCENT);
        }
    });

    // Separator
    ui.line(|ui| {
        for &w in &col_widths {
            ui.text(format!("{:─<w$}", "", w = w)).fg(DIM);
        }
    });

    // Rows
    for row in rows {
        ui.line(|ui| {
            for (i, cell) in row.iter().enumerate() {
                let w = col_widths.get(i).copied().unwrap_or(10);
                ui.text(format!("{:<w$}", cell, w = w));
            }
        });
    }
}
