//! First-run setup flow.
//!
//! Checks for existing configuration and, if none is found, walks the user
//! through API-key entry and endpoint selection.  Can also be invoked
//! explicitly via `vettd setup`.

use std::io::{self, BufRead, IsTerminal, Write};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};

use crate::network::ensure_endpoint_allowed;
use crate::submit::{load_auth_config, save_auth_config, AuthConfig, DEFAULT_PRODUCTION_ENDPOINT};

// ── ANSI constants ──────────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const INV: &str = "\x1b[7m";

const LOCAL_ENDPOINT: &str = "http://localhost:3000/api/scans/ingest";

// ── Prompt helpers (duplicated from wizard to keep modules independent) ──

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

fn ask(prompt: &str, default: &str) -> String {
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

fn pick(prompt: &str, options: &[&str], default: usize) -> usize {
    if !is_tty() {
        return pick_fallback(prompt, options, default);
    }

    let mut idx = default;
    terminal::enable_raw_mode().ok();

    loop {
        render_pick(prompt, options, idx);
        match read_key().as_str() {
            "up" | "k" if idx > 0 => idx -= 1,
            "down" | "j" if idx + 1 < options.len() => idx += 1,
            "enter" => break,
            "ctrl-c" | "esc" => {
                terminal::disable_raw_mode().ok();
                eprintln!("\r\x1b[K");
                eprintln!("  {DIM}Cancelled.{RESET}");
                std::process::exit(0);
            }
            _ => {}
        }
    }
    terminal::disable_raw_mode().ok();
    clear_pick(options.len());
    eprintln!("  {prompt}: {CYAN}{}{RESET}", options[idx]);
    idx
}

fn render_pick(prompt: &str, options: &[&str], selected: usize) {
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

fn clear_pick(option_count: usize) {
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

// ── Setup flow ──────────────────────────────────────────────────────────

/// Returns `true` if an API key is configured (connected mode).
/// Returns `false` if running in local-only mode.
pub fn ensure_configured() -> bool {
    if let Some(auth) = load_auth_config() {
        if !auth.api_key.is_empty() {
            return true;
        }
    }
    run_setup(false)
}

/// Run the interactive setup flow.
///
/// When `force` is true, runs even if a config already exists.
/// Returns `true` if an API key was configured, `false` for local-only.
pub fn run_setup(force: bool) -> bool {
    if !force {
        if let Some(auth) = load_auth_config() {
            if !auth.api_key.is_empty() {
                return true;
            }
        }
    }

    print_welcome();

    eprintln!("  {DIM}Enter your API key to sync scan results to Vettd");
    eprintln!("  for verification scoring and trend tracking, or");
    eprintln!("  press Enter to run in local-only mode.{RESET}");
    eprintln!();

    let key = crate::wizard::ask_secret("API key (or Enter to skip)");

    if key.is_empty() {
        eprintln!();
        eprintln!("  {GREEN}✓{RESET} {BOLD}Local-only mode{RESET}");
        eprintln!("  {DIM}Results will be saved to a JSON file.{RESET}");
        eprintln!("  {DIM}Run `vettd setup` later to configure an API key.{RESET}");
        eprintln!();
        return false;
    }

    let endpoints = &[
        "Vettd Cloud (vettd.agentichighway.ai)",
        "Local development server (localhost:3000)",
    ];
    let idx = pick("Endpoint", endpoints, 0);

    let endpoint = match idx {
        1 => LOCAL_ENDPOINT.to_string(),
        _ => DEFAULT_PRODUCTION_ENDPOINT.to_string(),
    };

    let config = AuthConfig {
        endpoint: endpoint.clone(),
        api_key: key,
    };
    // Both preset options (Vettd Cloud and localhost) are explicitly chosen by
    // the user — validate for defence in depth, allowing public since the
    // Vettd choice is a known, intentional opt-in.
    if let Err(e) = ensure_endpoint_allowed(&endpoint, true) {
        eprintln!();
        eprintln!("  Error: endpoint validation failed: {e}");
        eprintln!("  {DIM}Continuing in local-only mode.{RESET}");
        eprintln!();
        return false;
    }
    match save_auth_config(&config) {
        Ok(()) => {
            eprintln!();
            eprintln!("  {GREEN}✓{RESET} {BOLD}Configuration saved{RESET}");
            eprintln!("  {DIM}Endpoint: {endpoint}{RESET}");
            eprintln!();
        }
        Err(e) => {
            eprintln!();
            eprintln!("  Error saving config: {e}");
            eprintln!("  {DIM}Continuing in local-only mode.{RESET}");
            eprintln!();
            return false;
        }
    }

    true
}

fn print_welcome() {
    eprintln!();
    eprintln!("  {DIM}┌──────────────────────────────────────────┐{RESET}");
    eprintln!("  {DIM}│{RESET}  {BOLD}{CYAN}vettd{RESET}  —  First-Time Setup               {DIM}│{RESET}");
    eprintln!("  {DIM}└──────────────────────────────────────────┘{RESET}");
    eprintln!();
}
