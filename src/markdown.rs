use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, parse_document};

use crate::render::Block;

/// Parse markdown string into a list of renderable blocks
pub fn parse(content: &str) -> Vec<Block> {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.math_dollars = true;
    options.extension.math_code = true;

    let root = parse_document(&arena, content, &options);
    let mut blocks = Vec::new();
    collect_blocks(root, &mut blocks);
    blocks
}

fn collect_blocks<'a>(node: &'a AstNode<'a>, blocks: &mut Vec<Block>) {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::Document => {
            for child in node.children() {
                collect_blocks(child, blocks);
            }
        }
        NodeValue::Heading(heading) => {
            let text = collect_text(node);
            blocks.push(Block::Heading {
                level: heading.level,
                text,
            });
        }
        NodeValue::Paragraph => {
            let inlines = collect_inlines(node);
            blocks.push(Block::Paragraph { inlines });
        }
        NodeValue::CodeBlock(code) => {
            let lang = code.info.trim().to_string();
            let literal = code.literal.clone();
            if lang == "mermaid" {
                blocks.push(Block::Mermaid {
                    source: literal,
                    image: None,
                });
            } else {
                blocks.push(Block::CodeBlock {
                    lang: if lang.is_empty() { None } else { Some(lang) },
                    code: literal,
                });
            }
        }
        NodeValue::List(_) => {
            let items = collect_list_items(node);
            blocks.push(Block::List { items });
        }
        NodeValue::BlockQuote => {
            let text = collect_text(node);
            blocks.push(Block::BlockQuote { text });
        }
        NodeValue::ThematicBreak => {
            blocks.push(Block::ThematicBreak);
        }
        NodeValue::Table(_) => {
            let (headers, rows) = collect_table(node);
            blocks.push(Block::Table { headers, rows });
        }
        NodeValue::Math(math) => {
            let literal = math.literal.clone();
            blocks.push(Block::Math {
                source: literal,
                display: math.display_math,
            });
        }
        _ => {
            for child in node.children() {
                collect_blocks(child, blocks);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Inline {
    Text(String),
    Bold(String),
    Italic(String),
    Strikethrough(String),
    Code(String),
    Link { text: String, url: String },
    Math { source: String },
    SoftBreak,
    LineBreak,
}

fn collect_inlines<'a>(node: &'a AstNode<'a>) -> Vec<Inline> {
    let mut inlines = Vec::new();
    collect_inlines_inner(node, &mut inlines, InlineCtx::Normal);
    inlines
}

#[derive(Clone, Copy)]
enum InlineCtx {
    Normal,
    Bold,
    Italic,
    Strikethrough,
}

fn collect_inlines_inner<'a>(node: &'a AstNode<'a>, inlines: &mut Vec<Inline>, ctx: InlineCtx) {
    for child in node.children() {
        let data = child.data.borrow();
        match &data.value {
            NodeValue::Text(text) => {
                let t = text.to_string();
                match ctx {
                    InlineCtx::Normal => inlines.push(Inline::Text(t)),
                    InlineCtx::Bold => inlines.push(Inline::Bold(t)),
                    InlineCtx::Italic => inlines.push(Inline::Italic(t)),
                    InlineCtx::Strikethrough => inlines.push(Inline::Strikethrough(t)),
                }
            }
            NodeValue::Code(code) => {
                inlines.push(Inline::Code(code.literal.clone()));
            }
            NodeValue::Strong => {
                collect_inlines_inner(child, inlines, InlineCtx::Bold);
            }
            NodeValue::Emph => {
                collect_inlines_inner(child, inlines, InlineCtx::Italic);
            }
            NodeValue::Strikethrough => {
                collect_inlines_inner(child, inlines, InlineCtx::Strikethrough);
            }
            NodeValue::Link(link) => {
                let text = collect_text(child);
                let url = link.url.clone();
                inlines.push(Inline::Link { text, url });
            }
            NodeValue::Math(math) => {
                inlines.push(Inline::Math {
                    source: math.literal.clone(),
                });
            }
            NodeValue::SoftBreak => {
                inlines.push(Inline::SoftBreak);
            }
            NodeValue::LineBreak => {
                inlines.push(Inline::LineBreak);
            }
            _ => {
                collect_inlines_inner(child, inlines, ctx);
            }
        }
    }
}

fn collect_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    collect_text_inner(node, &mut text);
    text
}

fn collect_text_inner<'a>(node: &'a AstNode<'a>, out: &mut String) {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::Text(t) => {
            out.push_str(t);
        }
        NodeValue::Code(c) => {
            out.push_str(&c.literal);
        }
        NodeValue::SoftBreak | NodeValue::LineBreak => {
            out.push(' ');
        }
        _ => {}
    }
    for child in node.children() {
        collect_text_inner(child, out);
    }
}

fn collect_list_items<'a>(node: &'a AstNode<'a>) -> Vec<String> {
    let mut items = Vec::new();
    for child in node.children() {
        let data = child.data.borrow();
        if matches!(data.value, NodeValue::Item(_)) {
            items.push(collect_text(child).trim().to_string());
        }
    }
    items
}

fn collect_table<'a>(node: &'a AstNode<'a>) -> (Vec<String>, Vec<Vec<String>>) {
    let mut headers = Vec::new();
    let mut rows = Vec::new();
    let mut is_header = true;

    for child in node.children() {
        let data = child.data.borrow();
        if let NodeValue::TableRow(header) = &data.value {
            let mut cells = Vec::new();
            for cell in child.children() {
                cells.push(collect_text(cell).trim().to_string());
            }
            if is_header || *header {
                headers = cells;
                is_header = false;
            } else {
                rows.push(cells);
            }
        }
    }
    (headers, rows)
}
