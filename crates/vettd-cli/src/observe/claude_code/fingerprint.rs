//! The local-only fingerprint of a tool input.
//!
//! Split out of `project.rs` for the file-length budget, and because its reasoning is separate from
//! everything else in the projection: this is the one place the port deliberately re-implements a
//! serialiser the workspace already has.

use std::fmt::Write as _;

use serde_json::Value;

use crate::observe::canonical::hex_sha256;

/// The local-only fingerprint of a tool input (`_sha256_json`, `claude_code.py:612-614`).
///
/// The preimage is Python's
/// `json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, default=str)`.
/// This is **not** [`crate::observe::canonical::canonical_json`] and must not become it: that
/// function rejects floats by design because the envelope has none, while a tool input is arbitrary
/// JSON from the harness and may well carry one. These bytes are hashed on this machine and never
/// egress — the hash exists only so two identical inputs in one run compare equal — so the cost of
/// the duplicated writer is bounded, and the alternative (widening the egress canonicaliser to
/// accept floats) would weaken the guarantee that file exists to make.
///
/// Named non-parity, all of it confined to floats:
/// * a float small enough to need a **single-digit negative exponent** renders differently —
///   CPython pads the exponent (`1e-05`, `1.23e-08`) where Rust's shortest round-trip formatting
///   writes `0.00001` and `1.23e-8`. Every other float shape agrees, positive exponents included
///   (`1e+30` on both sides), as does every integer, string, bool, null and container.
/// * `NaN`/`Infinity` cannot occur, because JSON cannot express them.
/// * `default=str` cannot fire, because every value came from `serde_json`.
///
/// The consequence of that one difference is nil even where it bites: a fingerprint is only ever
/// compared to other fingerprints from this same function, and both renderings are injective, so
/// two inputs group together here exactly when they group together in the prototype.
pub(crate) fn sha256_json(value: &Value) -> String {
    let mut out = String::new();
    write_fingerprint(value, &mut out);
    hex_sha256(out.as_bytes())
}

/// Render `value` as Python's `json.dumps(…, sort_keys=True, separators=(",", ":"),
/// ensure_ascii=True)` would. Object keys are already in sorted order: `serde_json::Map` is a
/// `BTreeMap` in this workspace, and UTF-8 byte order is code-point order.
pub(crate) fn write_fingerprint(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => write_ascii_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_fingerprint(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_ascii_string(key, out);
                out.push(':');
                write_fingerprint(item, out);
            }
            out.push('}');
        }
    }
}

/// Write `value` as a JSON string escaped the way CPython's `ensure_ascii=True` escapes: only
/// `0x20`-`0x7e` survive, astral characters become a UTF-16 surrogate pair, and the `\uXXXX` hex is
/// lowercase.
fn write_ascii_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            _ => write_escape(character, out),
        }
    }
    out.push('"');
}

/// Write one `\uXXXX` escape, or the surrogate pair for a character above the BMP.
fn write_escape(character: char, out: &mut String) {
    let code = character as u32;
    if let Ok(unit) = u16::try_from(code) {
        write!(out, "\\u{unit:04x}").expect("writing to a String is infallible");
        return;
    }
    let offset = code - 0x1_0000;
    let high = 0xD800 + ((offset >> 10) as u16);
    let low = 0xDC00 + ((offset & 0x3FF) as u16);
    write!(out, "\\u{high:04x}\\u{low:04x}").expect("writing to a String is infallible");
}

#[cfg(test)]
#[path = "fingerprint_tests.rs"]
mod tests;
