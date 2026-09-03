//! `vettd observe enable | status | check`.
//!
//! These three exist so the opt-in, the local state and the egress gate are all inspectable
//! without running an observation. `check` in particular is what lets someone audit a payload
//! they were handed, with no access to the machine that produced it.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::observe::gate::{Dynamic, GATE};
use crate::observe::pipeline::{EXIT_GATE, EXIT_OK, EXIT_RUNTIME};

/// `observe check` uses 2 for "could not read the input", distinct from 1 for "found violations".
const EXIT_UNREADABLE: i32 = 2;

const TELEMETRY_TABLE: &str = "[telemetry]";
const TELEMETRY_SNIPPET: &str = "\n[telemetry]\nenabled = true\n";

/// Record the opt-in, or say exactly what to change when the table already exists.
///
/// Never rewrites a user-authored file. If `[telemetry]` is already present — even set to `false` —
/// this prints the path and the line to change rather than editing around whatever else is in
/// there. Silently flipping a value a user deliberately set to `false` would be the worst possible
/// behaviour for a consent flag.
pub(crate) fn enable() -> i32 {
    let Some(path) = crate::cli::access_config_path() else {
        eprintln!("Unable to determine home directory — cannot locate ~/.vettd/.vettd.toml");
        return EXIT_RUNTIME;
    };
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.contains(TELEMETRY_TABLE) {
        println!("{} already has a [telemetry] table.", path.display());
        println!("Set this line to enable observation:");
        println!();
        println!("    enabled = true");
        return EXIT_OK;
    }

    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Failed to create {}: {e}", parent.display());
            return EXIT_RUNTIME;
        }
    }
    let updated = format!("{}{}", existing.trim_end(), TELEMETRY_SNIPPET);
    if let Err(e) = std::fs::write(&path, updated) {
        eprintln!("Failed to write {}: {e}", path.display());
        return EXIT_RUNTIME;
    }
    println!("Observation enabled in {}.", path.display());
    println!("Run `vettd observe --dry-run` to see what an observation would contain.");
    EXIT_OK
}

/// Where everything lives and whether observation is on.
pub(crate) fn status(json: bool) -> i32 {
    let config = crate::cli::access_config_path();
    let enabled = crate::cli::telemetry_enabled_from_config();
    let secret = crate::identity::default_observer_secret_path().ok();
    let store = crate::observe::store::default_store_path().ok();
    let cursors = store
        .as_ref()
        .filter(|path| path.exists())
        .and_then(|path| crate::observe::store::Store::open_at(path).ok())
        .and_then(|store| store.has_any_cursor().ok())
        .unwrap_or(false);

    let display = |path: &Option<std::path::PathBuf>| {
        path.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    if json {
        let payload = serde_json::json!({
            "enabled": enabled,
            "config_path": display(&config),
            "secret_path": display(&secret),
            "secret_present": secret.as_ref().is_some_and(|p| p.exists()),
            "store_path": display(&store),
            "store_present": store.as_ref().is_some_and(|p| p.exists()),
            "cursor_state": if cursors { "resumed" } else { "fresh" },
            "envelope_version": crate::observe::envelope::ENVELOPE_VERSION,
            "extractor_version": crate::observe::envelope::EXTRACTOR_VERSION,
            "gate_version": crate::observe::envelope::GATE_VERSION,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(text) => println!("{text}"),
            Err(e) => {
                eprintln!("Failed to render status: {e}");
                return EXIT_RUNTIME;
            }
        }
        return EXIT_OK;
    }

    println!(
        "observation: {}",
        if enabled { "enabled" } else { "not enabled" }
    );
    println!("  config:  {}", display(&config));
    println!(
        "  secret:  {} ({})",
        display(&secret),
        if secret.as_ref().is_some_and(|p| p.exists()) {
            "present"
        } else {
            "not yet created"
        }
    );
    println!(
        "  store:   {} ({})",
        display(&store),
        if cursors {
            "has resumable cursors"
        } else {
            "no cursors"
        }
    );
    println!(
        "  gate:    v{} for envelope {}",
        crate::observe::envelope::GATE_VERSION,
        crate::observe::envelope::ENVELOPE_VERSION
    );
    if !enabled {
        println!();
        println!("Run `vettd observe enable` to opt in.");
    }
    EXIT_OK
}

/// Gate-check a written payload.
///
/// 0 clean, 1 violations, 2 unreadable. The dynamic sets are optional because a receiver auditing
/// someone else's payload does not have the emitter's local vocabulary; without them the gate still
/// enforces every structural and value rule, just not the substring rule.
pub(crate) fn check(payload: &Path, dynamic: Option<&Path>) -> i32 {
    let text = match std::fs::read_to_string(payload) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("Cannot read {}: {e}", payload.display());
            return EXIT_UNREADABLE;
        }
    };
    if let Some(duplicate) = first_duplicate_key(&text) {
        // serde_json keeps the LAST value for a duplicated key, so a payload could carry a leak in
        // the first copy and a clean value in the second and still validate. That is precisely what
        // this check exists to catch, so it is a hard read failure rather than a violation.
        eprintln!(
            "Cannot check {}: duplicate key in JSON object ({duplicate})",
            payload.display()
        );
        return EXIT_UNREADABLE;
    }
    let envelope: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("Cannot parse {}: {e}", payload.display());
            return EXIT_UNREADABLE;
        }
    };

    let sets = match dynamic {
        None => Dynamic::empty(),
        Some(path) => match load_dynamic(path) {
            Ok(sets) => sets,
            Err(message) => {
                eprintln!("{message}");
                return EXIT_UNREADABLE;
            }
        },
    };

    let violations = GATE.check(&envelope, &sets);
    if violations.is_empty() {
        println!(
            "gate: OK ({} allowed leaf paths, 0 violations)",
            GATE.field_count()
        );
        return EXIT_OK;
    }
    eprintln!(
        "gate: {} violation(s) in {}:",
        violations.len(),
        payload.display()
    );
    for violation in &violations {
        eprintln!("  {violation}");
    }
    EXIT_GATE.min(1)
}

fn load_dynamic(path: &Path) -> Result<Dynamic, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("Cannot parse {}: {e}", path.display()))?;
    Dynamic::from_json(&value)
}

/// The first duplicated key in any object of `text`, as `"<key>"` with its object path.
///
/// A small streaming scan rather than a full parser: `serde_json` cannot report this because it
/// resolves duplicates silently, and pulling in a second JSON parser to find out would be a large
/// dependency for one rule. Tracks the key set per object depth and reports the first repeat.
fn first_duplicate_key(text: &str) -> Option<String> {
    let mut stack: Vec<BTreeMap<String, ()>> = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut pending_key: Option<String> = None;

    while let Some((_, c)) = chars.next() {
        match c {
            '{' => stack.push(BTreeMap::new()),
            '}' => {
                stack.pop();
                pending_key = None;
            }
            '[' => stack.push(BTreeMap::new()),
            ']' => {
                stack.pop();
            }
            '"' => {
                let literal = read_string(&mut chars)?;
                // A string is a key only when the next non-space character is a colon.
                let is_key = matches!(peek_significant(&mut chars), Some(':'));
                if is_key {
                    if let Some(top) = stack.last_mut() {
                        if top.insert(literal.clone(), ()).is_some() {
                            return Some(literal);
                        }
                    }
                    pending_key = Some(literal);
                }
            }
            _ => {}
        }
    }
    let _ = pending_key;
    None
}

/// Consume a JSON string literal whose opening quote was already read.
fn read_string(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> Option<String> {
    let mut out = String::new();
    while let Some((_, c)) = chars.next() {
        match c {
            '\\' => {
                // Keep the escape verbatim; exact unescaping is not needed to compare key identity
                // as long as it is applied consistently, and JSON forbids a raw quote inside.
                out.push('\\');
                if let Some((_, next)) = chars.next() {
                    out.push(next);
                }
            }
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

fn peek_significant(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> Option<char> {
    while let Some((_, c)) = chars.peek().copied() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        return Some(c);
    }
    None
}

#[cfg(test)]
#[path = "subcommands_tests.rs"]
mod tests;
