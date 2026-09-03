//! Canonical JSON bytes and the hash primitives built on top of them.
//!
//! The envelope that leaves this machine must be *byte*-reproducible: the field
//! gate, the golden fixtures and the server-side duplicate detection all compare
//! bytes, and the Python prototype
//! (`spikes/828-passive-observer/prototype/aggregate.py::to_json_bytes`) is the
//! reference. That function is
//! `json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False)`
//! plus a trailing newline, so [`canonical_json`] reimplements exactly those
//! semantics rather than leaning on `serde_json::to_string`, whose escaping
//! (non-ASCII passed through raw) is not the same.
//!
//! Floats are a hard error here. The envelope has no floats by construction —
//! every number in `telemetry-envelope.schema.json` is an integer — and a float
//! would both break byte parity (Python's `repr`-based float formatting differs
//! from Rust's) and slip past the gate's integer bounds checks. Making that loud
//! is deliberate. The one place the prototype canonicalises values that *could*
//! contain floats is the local-only tool-input fingerprint
//! (`sources/claude_code.py:613`, `sources/codex.py:484`); those bytes are hashed
//! on this machine and never egress, so they do not go through this function and
//! may use `serde_json`'s default formatting instead.
//!
//! "Every number is an integer" is necessary but not sufficient for parity:
//! Python's `int` is arbitrary-precision, while `serde_json::Value` tops out at
//! `u64::MAX`. The gate bounds `ms2`/`tokens2` at 1e21, roughly 54x that, so a
//! `sumsq` in that range cannot round-trip through `Value` at all —
//! `serde_json::to_value(10u128.pow(21))` is itself an `Err`. The envelope
//! builder owns that constraint (Phase 4 of `docs/vettd-observe-port-plan.md`);
//! here it only means the real precondition is that every integer fits
//! `i64::MIN..=u64::MAX`, not merely that it is an integer.

use std::fmt::Write as _;

use hmac::{Hmac, Mac};
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Renders `value` exactly as Python's
/// `json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)`.
///
/// The result is always ASCII. Returns `Err` if any number in the tree is a
/// float — see the module docs for why that is fatal rather than best-effort.
pub(crate) fn canonical_json(value: &Value) -> Result<String, String> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

/// Canonical bytes for the wire: [`canonical_json`] plus one trailing newline.
///
/// Panics if the rendering is not ASCII, which would mean [`write_string`] let a
/// non-ASCII byte through and the egress bytes no longer match the prototype's
/// `.encode("ascii")`.
pub(crate) fn to_json_bytes(value: &Value) -> Result<Vec<u8>, String> {
    let mut text = canonical_json(value)?;
    text.push('\n');
    assert!(
        text.is_ascii(),
        "canonical JSON must be ASCII: the escaper let a non-ASCII byte through"
    );
    Ok(text.into_bytes())
}

/// Lowercase hex SHA-256 of `bytes` — matches `hashlib.sha256(bytes).hexdigest()`.
pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Lowercase hex HMAC-SHA256 over the UTF-8 bytes of `message` — matches
/// `hmac.new(secret, message.encode("utf-8"), hashlib.sha256).hexdigest()`.
///
/// This is the pseudonym primitive: `run_id = HMAC(secret, "{harness}:{session_key}")`
/// and `name_hash = HMAC(secret, "{asset_type}:{name}")`. The secret never
/// egresses, which is what makes those ids unlinkable off this device.
pub(crate) fn hmac_sha256_hex(secret: &[u8], message: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret)
        .expect("HMAC-SHA256 accepts a key of any length, including the empty key");
    mac.update(message.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

fn write_value(value: &Value, out: &mut String) -> Result<(), String> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(number, out)?,
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => write_object(map, out)?,
    }
    Ok(())
}

fn write_object(map: &serde_json::Map<String, Value>, out: &mut String) -> Result<(), String> {
    // `serde_json::Map` is a `BTreeMap` in this workspace (no `preserve_order`
    // feature), so iteration is already in key order, which for `String` is UTF-8
    // byte order == code-point order == Python `sort_keys`. Sort explicitly anyway:
    // enabling that feature anywhere in the dependency graph would silently switch
    // iteration to insertion order and change the egress bytes.
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_unstable();
    out.push('{');
    for (index, key) in keys.into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_string(key, out);
        out.push(':');
        let item = map.get(key).expect("key came from this map");
        write_value(item, out)?;
    }
    out.push('}');
    Ok(())
}

fn write_number(number: &Number, out: &mut String) -> Result<(), String> {
    if let Some(value) = number.as_i64() {
        out.push_str(&value.to_string());
        return Ok(());
    }
    if let Some(value) = number.as_u64() {
        out.push_str(&value.to_string());
        return Ok(());
    }
    Err(format!(
        "canonical JSON carries integers only; got the non-integer number {number}. \
         The envelope has no floats by construction, so this is an upstream bug, not a value."
    ))
}

/// Escapes exactly like CPython's `json.encoder.ESCAPE_ASCII`, whose pattern is
/// `([\\"]|[^ -~])`: only `0x20..=0x7e` survives raw, `"` and `\` are backslashed,
/// the five short forms are used where they exist, and every other character
/// becomes a `\uxxxx` escape with lowercase hex.
///
/// Two consequences worth naming, because both are easy to get wrong. First,
/// `0x7f` (DEL) *is* escaped, as `\u007f` — it sits outside `[ -~]`. The
/// "Python leaves DEL raw" rule (repeated in `docs/vettd-observe-port-plan.md`)
/// is the `ensure_ascii=False` pattern `[\x00-\x1f\\"]`, which is not what the
/// prototype uses; verified against CPython 3.11's C and pure-Python encoders.
/// Second, characters above the BMP are emitted as a UTF-16 surrogate pair, so
/// `U+1F600` becomes `\ud83d\ude00` rather than one escape or raw UTF-8.
fn write_string(value: &str, out: &mut String) {
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
            _ => write_unicode_escape(character, out),
        }
    }
    out.push('"');
}

fn write_unicode_escape(character: char, out: &mut String) {
    let code = character as u32;
    if code <= 0xFFFF {
        push_escaped_unit(code as u16, out);
        return;
    }
    let offset = code - 0x1_0000;
    push_escaped_unit(0xD800 + ((offset >> 10) as u16), out);
    push_escaped_unit(0xDC00 + ((offset & 0x3FF) as u16), out);
}

fn push_escaped_unit(unit: u16, out: &mut String) {
    write!(out, "\\u{unit:04x}").expect("writing to a String is infallible");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(value: &Value) -> String {
        canonical_json(value).expect("canonical_json should accept this value")
    }

    /// Every escape CPython's `ensure_ascii=True` produces must be reproduced
    /// verbatim: the envelope is compared byte for byte against the goldens and by
    /// the server's duplicate detection, so a single differing escape is a wire
    /// break, not a cosmetic difference.
    ///
    /// Expectations produced by, for each input below:
    ///   python3 -c 'import json; print(json.dumps("é", sort_keys=True,
    ///     separators=(",",":"), ensure_ascii=True))'   ->   "\u00e9"
    #[test]
    fn canonical_json_matches_python_ensure_ascii() {
        assert_eq!(render(&json!("é")), r#""\u00e9""#);
        assert_eq!(render(&json!("a\"b\\c")), r#""a\"b\\c""#);
        assert_eq!(render(&json!("\u{8}\u{c}\n\r\t")), r#""\b\f\n\r\t""#);
        assert_eq!(render(&json!("\u{1}")), r#""\u0001""#);
        assert_eq!(render(&json!("\u{0}")), r#""\u0000""#);
        // No short form for 0x0b or 0x1f: Python spells them out, unlike \b and \f.
        assert_eq!(render(&json!("\u{b}")), r#""\u000b""#);
        assert_eq!(render(&json!("\u{1f}")), r#""\u001f""#);
        // `/` is never escaped, and every non-ASCII BMP char always is.
        assert_eq!(render(&json!("a/b")), r#""a/b""#);
        assert_eq!(render(&json!("ÿĀ\u{ffff}")), r#""\u00ff\u0100\uffff""#);
    }

    /// DEL is escaped, not passed through. `ensure_ascii=True` keeps only
    /// `0x20..=0x7e` raw, so `0x7f` becomes `\u007f`; emitting it raw would put a
    /// control byte on the wire and diverge from the prototype's bytes.
    ///
    /// The port plan says DEL stays raw. It does not — that is the
    /// `ensure_ascii=False` rule. Verified against CPython 3.11, both encoders:
    ///   python3 -c 'import json; print(json.dumps(chr(0x7f), sort_keys=True,
    ///     separators=(",",":"), ensure_ascii=True))'   ->   "\u007f"
    #[test]
    fn canonical_json_escapes_del_like_python() {
        assert_eq!(render(&json!("\u{7f}")), r#""\u007f""#);
        //   python3 -c 'import json; print(json.dumps("a\x7fb\x0b\x01é\U0001F600",
        //     sort_keys=True, separators=(",",":"), ensure_ascii=True))'
        assert_eq!(
            render(&json!("a\u{7f}b\u{b}\u{1}é😀")),
            r#""a\u007fb\u000b\u0001\u00e9\ud83d\ude00""#
        );
    }

    /// Astral characters must be split into a UTF-16 surrogate pair. A single
    /// `\U0001f600`-style escape, or raw UTF-8, would be valid JSON but different
    /// bytes — and the envelope's identity is its bytes.
    ///
    ///   python3 -c 'import json; print(json.dumps("😀", sort_keys=True,
    ///     separators=(",",":"), ensure_ascii=True))'   ->   "\ud83d\ude00"
    ///   ... the same call for "\U0010FFFF"             ->   "\udbff\udfff"
    #[test]
    fn canonical_json_escapes_astral_chars_as_surrogate_pairs() {
        assert_eq!(render(&json!("😀")), r#""\ud83d\ude00""#);
        assert_eq!(render(&json!("\u{10FFFF}")), r#""\udbff\udfff""#);
    }

    /// Object keys are ordered by code point (== UTF-8 byte order for the key
    /// strings), matching Python `sort_keys=True`. Determinism D3 says the same
    /// runs must produce identical bytes no matter what order they were discovered
    /// in, and key order is where that would leak first.
    ///
    ///   python3 -c 'import json; print(json.dumps({"b":1,"A":2,"a":3,"é":4,
    ///     "😀":5,"_":6,"":7}, sort_keys=True, separators=(",",":"),
    ///     ensure_ascii=True))'
    ///   ->   {"":7,"A":2,"_":6,"a":3,"b":1,"\u00e9":4,"\ud83d\ude00":5}
    #[test]
    fn canonical_json_sorts_object_keys_by_code_point() {
        let value = json!({"b": 1, "A": 2, "a": 3, "é": 4, "😀": 5, "_": 6, "": 7});
        assert_eq!(
            render(&value),
            r#"{"":7,"A":2,"_":6,"a":3,"b":1,"\u00e9":4,"\ud83d\ude00":5}"#
        );
    }

    /// Literals, integers and containers render with `,`/`:` separators and no
    /// whitespace. A stray space would change the payload hash even though the
    /// JSON stays semantically identical, so the separators are load-bearing.
    ///
    ///   python3 -c 'import json; print(json.dumps({"n":None,"t":True,"f":False,
    ///     "i":-42,"big":9223372036854775807}, sort_keys=True,
    ///     separators=(",",":"), ensure_ascii=True))'
    ///   ->   {"big":9223372036854775807,"f":false,"i":-42,"n":null,"t":true}
    #[test]
    fn canonical_json_renders_literals_and_integers_without_whitespace() {
        let value =
            json!({"n": null, "t": true, "f": false, "i": -42, "big": 9223372036854775807i64});
        assert_eq!(
            render(&value),
            r#"{"big":9223372036854775807,"f":false,"i":-42,"n":null,"t":true}"#
        );
        assert_eq!(render(&json!({"a": [], "b": {}})), r#"{"a":[],"b":{}}"#);
        assert_eq!(render(&json!([1, [2, [3]]])), "[1,[2,[3]]]");
    }

    /// A float anywhere in the tree is an error, not a rendering choice. The
    /// envelope schema has no floats, so one appearing means a caller built the
    /// wrong shape; formatting it silently would break byte parity with Python and
    /// walk past the gate's integer bounds checks.
    #[test]
    fn canonical_json_rejects_floats_anywhere() {
        assert!(canonical_json(&json!(1.5)).is_err());
        // A float-typed number whose value happens to be integral is still a float.
        assert!(canonical_json(&json!(1.0)).is_err());
        assert!(canonical_json(&json!([1, {"b": 0.25}])).is_err());
        assert!(canonical_json(&json!({"records": [{"tokens": {"input": -0.5}}]})).is_err());
    }

    /// The wire form is the canonical text plus exactly one `\n`, all ASCII. The
    /// prototype encodes with `.encode("ascii")` and the reader on the other end
    /// treats the payload as one newline-terminated ASCII document, so both the
    /// newline count and the ASCII property are part of the contract.
    ///
    ///   python3 -c 'import json; print((json.dumps({"a":1}, sort_keys=True,
    ///     separators=(",",":"), ensure_ascii=True) + "\n").encode("ascii"))'
    ///   ->   b'{"a":1}\n'
    #[test]
    fn to_json_bytes_ends_with_exactly_one_newline_and_is_ascii() {
        let bytes = to_json_bytes(&json!({"a": 1})).expect("integers are renderable");
        assert_eq!(bytes, b"{\"a\":1}\n".to_vec());

        let unicode = to_json_bytes(&json!({"k": "é😀"})).expect("strings are renderable");
        assert!(unicode.is_ascii());
        assert_eq!(unicode.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(
            String::from_utf8(unicode).expect("canonical bytes are ASCII"),
            concat!(r#"{"k":"\u00e9\ud83d\ude00"}"#, "\n")
        );

        assert!(to_json_bytes(&json!({"a": 1.5})).is_err());
    }

    /// SHA-256 digests must match `hashlib` exactly: `bom_version` is
    /// `sha256(",".join(sorted(set(asset_ids))))` and the skill tree hash is
    /// `sha256(canonical_json(pairs))`, so a mismatch would make every locally
    /// computed asset identity disagree with the prototype and with the goldens.
    ///
    ///   python3 -c 'import hashlib; print(hashlib.sha256(b"a,b,c").hexdigest())'
    #[test]
    fn hex_sha256_matches_python_hashlib() {
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // The `bom_version` preimage shape: sorted, de-duplicated asset ids joined by ",".
        assert_eq!(
            hex_sha256(b"a,b,c"),
            "205830ca5b23bbe39ab510cfddc1dff2d9842e38b5fa7b7c48cd4ca7e44f92a1"
        );
        // The skill tree-hash shape: sha256 over the canonical bytes of sorted pairs.
        //   python3 -c 'import hashlib,json; print(hashlib.sha256(json.dumps(
        //     [["a.md","ff"],["b.md","ee"]], sort_keys=True, separators=(",",":"),
        //     ensure_ascii=True).encode("utf-8")).hexdigest())'
        let pairs = json!([["a.md", "ff"], ["b.md", "ee"]]);
        assert_eq!(render(&pairs), r#"[["a.md","ff"],["b.md","ee"]]"#);
        assert_eq!(
            hex_sha256(render(&pairs).as_bytes()),
            "de23da9f45bba70c171be230a69c27397525b5c5db9561628588c9e9707d06b5"
        );
    }

    /// HMAC pseudonyms must match `hmac.new(secret, msg, sha256).hexdigest()` byte
    /// for byte, including the UTF-8 encoding of the message: `run_id` is the
    /// server's primary key for a run, so a different digest would land the same
    /// run twice instead of being recognised as a duplicate.
    ///
    ///   python3 -c 'import hashlib,hmac; print(hmac.new(b"secret",
    ///     "claude-code:sess-1".encode("utf-8"), hashlib.sha256).hexdigest())'
    #[test]
    fn hmac_sha256_hex_matches_python_hmac() {
        // run_id preimage: "{harness}:{session_key}" (aggregate.py:78-91).
        assert_eq!(
            hmac_sha256_hex(b"secret", "claude-code:sess-1"),
            "c31502034c375ab6a56ff299ef19532d97047e6c8d34790575d5ac179719a809"
        );
        // name_hash preimage: "{asset_type}:{name}" (attribute.py:74-84).
        assert_eq!(
            hmac_sha256_hex(b"secret", "skill:my-skill"),
            "e2b962ac21d6cbed0308ab255d63afcab713b8d68cfc36cbbf3b0e5799ed0d6b"
        );
        // A non-ASCII asset name is hashed as UTF-8, never as its escaped JSON form.
        assert_eq!(
            hmac_sha256_hex(b"secret", "é:😀"),
            "81f4dae6fa0e05650d4ea47046e2b18aa4a78b80df74a20db6ff04f9980622bf"
        );
        // An empty key must not panic: HMAC accepts keys of any length.
        assert_eq!(
            hmac_sha256_hex(b"", ""),
            "b613679a0814d9ec772f95d778c35fc5ff1697c493715653c6c712144292c5ad"
        );
    }
}
