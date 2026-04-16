mod catmode;
mod markdown;
mod render;

use clap::Parser;
use notify::{Event, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use slt::*;

#[derive(Parser)]
#[command(name = "tmp", about = "Terminal Markdown Preview")]
struct Cli {
    /// Markdown file to preview
    file: PathBuf,

    /// Disable live reload (file watching)
    #[arg(long)]
    no_watch: bool,

    /// Force TUI mode (even on non-Kitty terminals)
    #[arg(long)]
    tui: bool,

    /// Force cat mode (stdout output, no TUI)
    #[arg(long)]
    cat: bool,
}

/// Check if terminal supports Kitty graphics protocol
fn is_kitty_terminal() -> bool {
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

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    let content = std::fs::read_to_string(&cli.file).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {e}", cli.file.display());
        std::process::exit(1);
    });

    // Mode selection: --tui / --cat override auto-detect
    let use_tui = if cli.tui {
        true
    } else if cli.cat {
        false
    } else {
        is_kitty_terminal()
    };

    if use_tui {
        run_tui(cli, content)
    } else {
        run_cat(cli, content)
    }
}

/// Cat mode: render to stdout, native terminal scrolling, pixel-perfect images
fn run_cat(cli: Cli, mut content: String) -> std::io::Result<()> {
    catmode::render_to_stdout(&content);

    if cli.no_watch {
        return Ok(());
    }

    // Watch for changes and re-render
    let (tx, rx) = mpsc::channel::<()>();
    let watch_path = cli.file.canonicalize().unwrap_or(cli.file.clone());

    let _watcher = {
        let tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res
                && event.kind.is_modify()
            {
                let _ = tx.send(());
            }
        })
        .ok();
        if let Some(ref mut w) = watcher {
            let _ = w.watch(&watch_path, RecursiveMode::NonRecursive);
        }
        watcher
    };

    // Also watch for terminal resize via polling
    let mut last_reload = Instant::now();
    let mut last_size = crossterm::terminal::size().unwrap_or((80, 24));

    loop {
        // Non-blocking: check file changes or timeout for resize check
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(()) => {
                if last_reload.elapsed() > Duration::from_millis(200) {
                    if let Ok(new_content) = std::fs::read_to_string(&cli.file) {
                        content = new_content;
                        print!("\x1b[2J\x1b[H");
                        catmode::render_to_stdout(&content);
                        last_reload = Instant::now();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Check for terminal resize
                let new_size = crossterm::terminal::size().unwrap_or((80, 24));
                if new_size != last_size {
                    last_size = new_size;
                    print!("\x1b[2J\x1b[H");
                    catmode::render_to_stdout(&content);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

/// TUI mode: SLT with Kitty graphics, interactive scrolling
fn run_tui(cli: Cli, content: String) -> std::io::Result<()> {
    let mut md_content = content;
    let mut scroll = ScrollState::new();
    let mut needs_reparse = true;
    let mut rendered_blocks: Vec<render::Block> = Vec::new();

    let mut mermaid_rx: Option<mpsc::Receiver<(usize, render::RenderedImage)>> = None;

    let (tx, rx) = mpsc::channel::<()>();
    let watch_path = cli.file.canonicalize().unwrap_or(cli.file.clone());

    let _watcher = if !cli.no_watch {
        let tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res
                && event.kind.is_modify()
            {
                let _ = tx.send(());
            }
        })
        .ok();
        if let Some(ref mut w) = watcher {
            let _ = w.watch(&watch_path, RecursiveMode::NonRecursive);
        }
        watcher
    } else {
        None
    };

    let file_path = cli.file.clone();
    let mut last_reload = Instant::now();

    let mut config = RunConfig::default();
    config.mouse = true;

    slt::run_with(config, move |ui: &mut Context| {
        if rx.try_recv().is_ok()
            && last_reload.elapsed() > Duration::from_millis(100)
            && let Ok(new_content) = std::fs::read_to_string(&file_path)
        {
            md_content = new_content;
            needs_reparse = true;
            last_reload = Instant::now();
        }

        if needs_reparse {
            rendered_blocks = render::render_markdown_fast(&md_content);
            mermaid_rx = Some(render::render_mermaid_async(&rendered_blocks));
            needs_reparse = false;
        }

        if let Some(ref rx) = mermaid_rx {
            while let Ok((idx, img)) = rx.try_recv() {
                if let Some(render::Block::Mermaid { image, .. }) = rendered_blocks.get_mut(idx) {
                    *image = Some(img);
                }
            }
        }

        if ui.key('q') || ui.key_code(KeyCode::Esc) {
            ui.quit();
        }

        let _ = ui.scrollable(&mut scroll).grow(1).px(3).py(1).col(|ui| {
            render::render_blocks(ui, &rendered_blocks);
        });
    })
}
