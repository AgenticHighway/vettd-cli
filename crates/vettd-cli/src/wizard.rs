//! Interactive wizard for vettd.
//!
//! When invoked with no subcommand in a TTY, the wizard collects the scan
//! mode (and optional path) interactively, then returns a `Commands` variant
//! so the main pipeline in `cli::run()` handles everything from there.
//!
//! Uses crossterm for raw key input when running in a TTY; falls back to
//! numbered text menus otherwise.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};

use crate::cli::{OutputArgs, ScanSubcommand};

// ── ANSI constants ──────────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const INV: &str = "\x1b[7m";

// ── Low-level key reading ───────────────────────────────────────────────

fn is_tty() -> bool {
    io::stdin().is_terminal()
}

fn read_key() -> String {
    loop {
        if let Ok(Event::Key(KeyEvent {
            code, modifiers, ..
        })) = event::read()
        {
            if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
                return "ctrl-c".to_string();
            }
            return match code {
                KeyCode::Up => "up".to_string(),
                KeyCode::Down => "down".to_string(),
                KeyCode::Left => "left".to_string(),
                KeyCode::Right => "right".to_string(),
                KeyCode::Enter => "enter".to_string(),
                KeyCode::Esc => "esc".to_string(),
                KeyCode::Char(c) => c.to_string(),
                _ => continue,
            };
        }
    }
}

// ── Prompt helpers (pub(crate) so cli.rs can reuse them) ────────────────

pub(crate) fn ask(prompt: &str, default: &str) -> String {
    if !is_tty() {
        return default.to_string();
    }
    if default.is_empty() {
        eprint!("  {prompt}: ");
    } else {
        eprint!("  {prompt} [{default}]: ");
    }
    let _ = io::stderr().flush();

    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        return default.to_string();
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn ask_secret(prompt: &str) -> String {
    if !is_tty() {
        return ask(prompt, "");
    }

    eprint!("  {prompt}: ");
    let _ = io::stderr().flush();

    terminal::enable_raw_mode().ok();
    let mut secret = String::new();

    loop {
        match event::read() {
            Ok(Event::Key(KeyEvent {
                code, modifiers, ..
            })) => match code {
                KeyCode::Enter => break,
                KeyCode::Backspace => {
                    secret.pop();
                }
                KeyCode::Esc => {
                    terminal::disable_raw_mode().ok();
                    graceful_exit();
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    terminal::disable_raw_mode().ok();
                    graceful_exit();
                }
                KeyCode::Char(c) => secret.push(c),
                _ => {}
            },
            Ok(_) => {}
            Err(_) => break,
        }
    }

    terminal::disable_raw_mode().ok();
    eprintln!();
    secret
}

pub(crate) fn confirm(prompt: &str, default: bool) -> bool {
    if !is_tty() {
        let hint = if default { "Y/n" } else { "y/N" };
        let ans = ask(&format!("{prompt} ({hint})"), "");
        return match ans.to_lowercase().as_str() {
            "y" | "yes" => true,
            "n" | "no" => false,
            _ => default,
        };
    }

    let mut value = default;
    terminal::enable_raw_mode().ok();

    loop {
        let yes_style = if value {
            format!("{INV} Yes {RESET}")
        } else {
            " Yes ".to_string()
        };
        let no_style = if !value {
            format!("{INV} No {RESET}")
        } else {
            " No ".to_string()
        };
        eprint!("\r\x1b[K  {prompt}  {yes_style}  {no_style}");
        let _ = io::stderr().flush();

        match read_key().as_str() {
            "left" | "right" | "y" | "n" => value = !value,
            "enter" => break,
            "ctrl-c" | "esc" => {
                terminal::disable_raw_mode().ok();
                graceful_exit();
            }
            _ => {}
        }
    }
    terminal::disable_raw_mode().ok();
    eprintln!();
    value
}

pub(crate) fn pick(prompt: &str, options: &[&str], default: usize) -> usize {
    if !is_tty() {
        return pick_fallback(prompt, options, default);
    }

    let mut idx = default;
    terminal::enable_raw_mode().ok();

    loop {
        render_pick_menu(prompt, options, idx);
        match read_key().as_str() {
            "up" | "k" if idx > 0 => idx -= 1,
            "down" | "j" if idx + 1 < options.len() => idx += 1,
            "enter" => break,
            "ctrl-c" | "esc" => {
                terminal::disable_raw_mode().ok();
                graceful_exit();
            }
            _ => {}
        }
    }
    terminal::disable_raw_mode().ok();
    clear_pick_menu(options.len());
    eprintln!("  {prompt}: {CYAN}{}{RESET}", options[idx]);
    idx
}

fn render_pick_menu(prompt: &str, options: &[&str], selected: usize) {
    eprint!("\r\x1b[K  {BOLD}{prompt}{RESET}\r\n");
    for (i, opt) in options.iter().enumerate() {
        if i == selected {
            eprint!("\r\x1b[K    {CYAN}❯{RESET} {INV} {opt} {RESET}\r\n");
        } else {
            eprint!("\r\x1b[K      {DIM}{opt}{RESET}\r\n");
        }
    }
    let up = options.len() + 1;
    eprint!("\x1b[{up}A");
    let _ = io::stderr().flush();
}

fn clear_pick_menu(option_count: usize) {
    for _ in 0..=option_count {
        eprint!("\r\x1b[K\r\n");
    }
    let up = option_count + 1;
    eprint!("\x1b[{up}A");
    let _ = io::stderr().flush();
}

fn pick_fallback(prompt: &str, options: &[&str], default: usize) -> usize {
    eprintln!("  {prompt}:");
    for (i, opt) in options.iter().enumerate() {
        let marker = if i == default { " (default)" } else { "" };
        eprintln!("    {}: {opt}{marker}", i + 1);
    }
    let ans = ask("Choice", &(default + 1).to_string());
    ans.parse::<usize>()
        .ok()
        .filter(|&v| v >= 1 && v <= options.len())
        .map(|v| v - 1)
        .unwrap_or(default)
}

fn graceful_exit() -> ! {
    eprintln!("\r\x1b[K");
    eprintln!("  {DIM}Cancelled.{RESET}");
    std::process::exit(0);
}

// ── Banner ──────────────────────────────────────────────────────────────

fn print_banner() {
    eprintln!();
    eprintln!("  {DIM}┌──────────────────────────────────────────┐{RESET}");
    eprintln!(
        "  {DIM}│{RESET}  {BOLD}{CYAN}vettd{RESET}  —  AI Execution Inventory        {DIM}│{RESET}"
    );
    eprintln!("  {DIM}└──────────────────────────────────────────┘{RESET}");
    eprintln!();
}

// ── Public entry point ──────────────────────────────────────────────────

/// Show the interactive banner and prompt for a scan mode.
/// Returns a `ScanSubcommand` with default `OutputArgs` so the main
/// pipeline handles output/submit/contract-sync identically.
pub fn pick_scan() -> ScanSubcommand {
    print_banner();

    let modes = &[
        "Default scan  (home directory)",
        "Quick scan    (agentic config areas)",
        "Full scan     (entire filesystem)",
        "Folder scan   (specific directory)",
        "File scan     (single file)",
    ];
    let idx = pick("Scan mode", modes, 0);

    let output = OutputArgs::default();

    match idx {
        0 => ScanSubcommand::Default { output },
        1 => ScanSubcommand::Quick { output },
        2 => ScanSubcommand::Full { output },
        3 => {
            let dir = ask("Directory path", ".");
            ScanSubcommand::Folder {
                path: PathBuf::from(dir),
                deep: false,
                output,
            }
        }
        4 => {
            let path = ask("File path", "");
            ScanSubcommand::File {
                path: PathBuf::from(path),
                output,
            }
        }
        _ => ScanSubcommand::Default { output },
    }
}
