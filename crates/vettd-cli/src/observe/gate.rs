//! Egress field gate for the passive-observer telemetry envelope.
//!
//! A direct port of `spikes/828-passive-observer/prototype/check_field_gate.py`, which stays the
//! reference semantics. Every leaf path a payload writes must be listed in the repo-root
//! `telemetry-field-gate.json`; nullable objects may be null, otherwise their children are checked
//! as leaves; enums are closed; formats and numeric bounds are enforced per field; hash, day and
//! allowed-uuid and decimal-integer paths are checked by exact format only; every other string
//! leaf must clear every `forbiddenValuePatterns` rule and every dynamic forbid set the emitter
//! hands over.
//!
//! A key-path walker cannot see a path, a URL, a uuid or a name inside a string; the value-level
//! rules are what make "logs never leave the machine" checkable on the payload rather than on the
//! code. Violation strings name the concrete instance path (`records[0].assets[1].asset_id`) so a
//! failing record can be found, and never echo a string value or an unknown key: either could be
//! exactly the local-only content the gate exists to withhold. [`Gate::check`] is pure.
//!
//! Deliberate divergences from the Python, none of them a weakening:
//! - the Python raises `ValueError` out of `check()` when the gate document itself is malformed (an
//!   unsupported field type, an undefined enum or format, a unit with no `numericBounds` entry);
//!   [`Gate::from_json`] rejects those eagerly, which keeps `check` infallible.
//! - `serde_json::Map` is a `BTreeMap` in this workspace, so object keys are visited in sorted
//!   order rather than in document order. The set of violations is identical, only the order of the
//!   returned lines differs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use regex::Regex;
use serde_json::{Map, Value};

/// The repo-root egress allowlist, compiled in so a gate check never depends on the filesystem.
const GATE_JSON: &str = include_str!("../../../../telemetry-field-gate.json");

/// The gate every emitter path checks against, parsed once.
pub(crate) static GATE: LazyLock<Gate> = LazyLock::new(|| {
    Gate::from_json(GATE_JSON).expect("compiled-in telemetry-field-gate.json is a valid gate")
});

/// Unix seconds and unix milliseconds, as ranges a plausible count could not otherwise land in.
const EPOCH_RANGES: [(f64, f64); 2] = [(1.5e9, 2.5e9), (1.5e12, 2.5e12)];
/// Dynamic-forbid entries shorter than this are skipped: they would match inside almost any value.
const DYNAMIC_MIN_LEN: usize = 3;
/// Dynamic sets whose entries are path-like and are therefore also split into components.
const COMPONENT_SETS: [&str; 3] = ["cwd_and_branches", "slugs", "home_dir"];
/// A path component shorter than this is too generic to forbid on its own.
const COMPONENT_MIN_LEN: usize = 4;
/// The one `forbiddenValuePatterns` entry the `regex` crate cannot compile (it uses lookaround).
const EPOCH_PATTERN_ID: &str = "epoch_in_string";
/// The gate's `whitespace` pattern is the bare `\s`, which the two engines disagree about.
const WHITESPACE_PATTERN_ID: &str = "whitespace";
/// The exact-format path lists, each naming the format its paths must match in full.
const EXACT_PATH_LISTS: [(&str, &str); 4] = [
    ("hex64", "hashPaths"),
    ("day", "dayPaths"),
    ("uuid", "allowedUuidPaths"),
    ("sumsq_decimal", "decimalIntegerPaths"),
];

static KEY_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A[A-Za-z_][A-Za-z0-9_]*\z").expect("KEY_NAME_RE is valid"));

static COMPONENT_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[/\\:._-]+").expect("COMPONENT_SPLIT_RE is valid"));

/// The four leaf types the gate can declare.
#[derive(Clone, Copy, Debug)]
enum FieldType {
    Boolean,
    Integer,
    String,
    Object,
}

impl FieldType {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "boolean" => Some(Self::Boolean),
            "integer" => Some(Self::Integer),
            "string" => Some(Self::String),
            "object" => Some(Self::Object),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::String => "string",
            Self::Object => "object",
        }
    }
}

/// One `fields` entry, reduced to what the checker consults.
struct FieldSpec {
    ty: FieldType,
    unit: Option<String>,
    format: Option<String>,
    enum_name: Option<String>,
    nullable: bool,
    required: bool,
}

/// A compiled forbidden-value rule.
enum Pattern {
    /// The gate's own regex, compiled by the `regex` crate.
    Regex(Regex),
    /// A hand-written equivalent for a gate regex the `regex` crate cannot compile.
    Fn(fn(&str) -> bool),
}

impl Pattern {
    /// Python uses `re.search`, so both arms are substring tests, never anchored matches.
    fn is_match(&self, value: &str) -> bool {
        match self {
            Self::Regex(re) => re.is_match(value),
            Self::Fn(f) => f(value),
        }
    }
}

/// Equivalent of the gate's `epoch_in_string` regex, which the `regex` crate cannot compile:
/// `(?<![0-9])1[5-9][0-9]{8}(?:[0-9]{3})?(?![0-9])`.
///
/// The match is all digits and may touch no digit on either side, so it is exactly a maximal run of
/// ASCII digits of length 10 (unix seconds) or 13 (unix milliseconds) that starts `1` and whose
/// second digit is `5`..=`9`. An 11- or 12-digit run cannot match: the shorter alternative would be
/// followed by a digit, and every interior start position is preceded by one.
fn epoch_in_string(value: &str) -> bool {
    value.as_bytes().split(|b| !b.is_ascii_digit()).any(|run| {
        matches!(run.len(), 10 | 13) && run[0] == b'1' && (b'5'..=b'9').contains(&run[1])
    })
}

/// Equivalent of the gate's `whitespace` pattern, `\s`, as Python's `re` reads it.
///
/// For `str` patterns CPython's `\s` is exactly `str.isspace()`, which is the Unicode `White_Space`
/// property *plus* the four field/group/record/unit separators U+001C..U+001F. The `regex` crate's
/// `\s` is `White_Space` alone, so compiling the gate's source verbatim would let those four
/// control characters through the one pattern whose stated job is to catch "any free text".
fn python_whitespace(value: &str) -> bool {
    value
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}'))
}

/// The emitter's local-only strings, normalised once into case-folded substring needles.
pub(crate) struct Dynamic {
    /// Set name to its sorted, deduplicated, lower-cased needles.
    sets: BTreeMap<String, Vec<String>>,
}

impl Dynamic {
    /// No dynamic sets at all — the Python's `dynamic=None`.
    pub(crate) fn empty() -> Self {
        Self {
            sets: BTreeMap::new(),
        }
    }

    /// Port of `_normalize_dynamic`: lower-case the emitter's sets, drop empty sets and entries
    /// shorter than [`DYNAMIC_MIN_LEN`]. Path-like sets ([`COMPONENT_SETS`]) are also split into
    /// their components (a branch leaf, a directory name, a slug word) so a value that carries only
    /// part of a path is still caught.
    pub(crate) fn normalize(sets: &BTreeMap<String, BTreeSet<String>>) -> Self {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, values) in sets {
            let mut needles: BTreeSet<String> = BTreeSet::new();
            for value in values {
                if value.chars().count() >= DYNAMIC_MIN_LEN {
                    needles.insert(value.to_lowercase());
                }
                if COMPONENT_SETS.contains(&name.as_str()) {
                    for part in COMPONENT_SPLIT_RE.split(value) {
                        if part.chars().count() >= COMPONENT_MIN_LEN {
                            needles.insert(part.to_lowercase());
                        }
                    }
                }
            }
            if !needles.is_empty() {
                out.insert(name.clone(), needles.into_iter().collect());
            }
        }
        Self { sets: out }
    }

    /// Parse `{set_name: [local-only strings]}` as the emitter writes it beside a payload. A set
    /// that is not a list of strings is an error, never silently ignored.
    pub(crate) fn from_json(value: &Value) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self::empty());
        }
        let object = value.as_object().ok_or_else(|| {
            "dynamic file must be a JSON object of {set_name: [strings]}".to_string()
        })?;
        let mut sets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (name, values) in object {
            let items = values
                .as_array()
                .ok_or_else(|| format!("dynamic set '{name}' must be a list of strings"))?;
            let mut entries = BTreeSet::new();
            for item in items {
                let text = item
                    .as_str()
                    .ok_or_else(|| format!("dynamic set '{name}' holds a non-string entry"))?;
                entries.insert(text.to_string());
            }
            sets.insert(name.clone(), entries);
        }
        Ok(Self::normalize(&sets))
    }
}

/// Lookups precomputed from one gate document.
pub(crate) struct Gate {
    fields: BTreeMap<String, FieldSpec>,
    enums: BTreeMap<String, BTreeSet<String>>,
    formats: BTreeMap<String, Regex>,
    bounds: BTreeMap<String, (f64, f64)>,
    /// Exact-format paths mapped to the format name each must match.
    exact_paths: BTreeMap<String, String>,
    /// Forbidden-value rules in gate order, so violations are reported in gate order.
    patterns: Vec<(String, Pattern)>,
    /// Every proper prefix of a field path: the intermediate objects a payload may write.
    object_paths: BTreeSet<String>,
    /// Per object path, the child keys that must be present when that object is written.
    required_children: BTreeMap<String, BTreeSet<String>>,
}

impl Gate {
    /// Parse and validate one gate document. Every reference the checker would follow at check time
    /// (a field's `format`, `enum` and `unit`, and each exact-format path) is resolved here, so a
    /// malformed gate fails loudly at load rather than quietly at check.
    pub(crate) fn from_json(text: &str) -> Result<Self, String> {
        let doc: Value =
            serde_json::from_str(text).map_err(|e| format!("gate is not valid JSON: {e}"))?;
        let fields = parse_fields(&doc)?;
        let (object_paths, required_children) = derive_structure(&fields);
        let gate = Self {
            enums: parse_enums(&doc)?,
            formats: parse_formats(&doc)?,
            bounds: parse_bounds(&doc)?,
            exact_paths: parse_exact_paths(&doc)?,
            patterns: parse_patterns(&doc)?,
            fields,
            object_paths,
            required_children,
        };
        gate.validate_references()?;
        Ok(gate)
    }

    fn validate_references(&self) -> Result<(), String> {
        for spec in self.fields.values() {
            if let Some(name) = &spec.format {
                if !self.formats.contains_key(name) {
                    return Err(format!("gate format '{name}' is not defined"));
                }
            }
            if let Some(name) = &spec.enum_name {
                if !self.enums.contains_key(name) {
                    return Err(format!("gate enum '{name}' is not defined"));
                }
            }
            if let Some(unit) = &spec.unit {
                if !self.bounds.contains_key(unit) {
                    return Err(format!("gate unit '{unit}' has no numericBounds entry"));
                }
            }
        }
        for name in self.exact_paths.values() {
            if !self.formats.contains_key(name) {
                return Err(format!("gate format '{name}' is not defined"));
            }
        }
        Ok(())
    }

    /// Return every violation of this gate in `payload`; an empty vector is a pass.
    ///
    /// `dynamic` carries the local-only strings the emitter saw while parsing; every set given is
    /// enforced, whether or not the gate lists its name under `dynamicForbids.sets`.
    /// How many leaf paths the gate admits.
    ///
    /// Reported alongside a clean result so the number in "gate: OK (N allowed leaf paths)" comes
    /// from the gate the binary actually compiled in, not from a constant someone has to remember
    /// to update.
    pub(crate) fn field_count(&self) -> usize {
        self.fields.len()
    }

    pub(crate) fn check(&self, payload: &Value, dynamic: &Dynamic) -> Vec<String> {
        let mut checker = Checker {
            gate: self,
            dynamic,
            violations: Vec::new(),
        };
        checker.walk(payload, "", "");
        checker.violations
    }
}

fn parse_fields(doc: &Value) -> Result<BTreeMap<String, FieldSpec>, String> {
    let object = doc
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| "gate has no 'fields' object".to_string())?;
    let mut fields = BTreeMap::new();
    for (path, spec) in object {
        let ty_name = spec
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("gate field '{path}' has no type"))?;
        let ty = FieldType::parse(ty_name)
            .ok_or_else(|| format!("gate field '{path}' has unsupported type '{ty_name}'"))?;
        fields.insert(
            path.clone(),
            FieldSpec {
                ty,
                unit: string_field(spec, "unit"),
                format: string_field(spec, "format"),
                enum_name: string_field(spec, "enum"),
                nullable: spec
                    .get("nullable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                required: spec
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            },
        );
    }
    Ok(fields)
}

fn string_field(spec: &Value, key: &str) -> Option<String> {
    spec.get(key).and_then(Value::as_str).map(str::to_string)
}

/// What [`derive_structure`] reads off the field list: the intermediate object paths, and the
/// required child keys per object path.
type Structure = (BTreeSet<String>, BTreeMap<String, BTreeSet<String>>);

/// Every proper prefix of a field path is an object path; every segment of a required field path is
/// a required child of its parent (`records[].tokens.input` requires `records` at the root,
/// `tokens` under `records[]`, and `input` under `records[].tokens`).
fn derive_structure(fields: &BTreeMap<String, FieldSpec>) -> Structure {
    let mut object_paths = BTreeSet::new();
    let mut required_children: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (path, spec) in fields {
        let segments: Vec<&str> = path.split('.').collect();
        for i in 1..segments.len() {
            object_paths.insert(segments[..i].join("."));
        }
        if !spec.required {
            continue;
        }
        for i in 0..segments.len() {
            let parent = segments[..i].join(".");
            let child = segments[i].strip_suffix("[]").unwrap_or(segments[i]);
            required_children
                .entry(parent)
                .or_default()
                .insert(child.to_string());
        }
    }
    (object_paths, required_children)
}

fn parse_enums(doc: &Value) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut enums = BTreeMap::new();
    let Some(object) = doc.get("enums").and_then(Value::as_object) else {
        return Ok(enums);
    };
    for (name, members) in object {
        let items = members
            .as_array()
            .ok_or_else(|| format!("gate enum '{name}' is not a list"))?;
        let mut set = BTreeSet::new();
        for item in items {
            let text = item
                .as_str()
                .ok_or_else(|| format!("gate enum '{name}' holds a non-string member"))?;
            set.insert(text.to_string());
        }
        enums.insert(name.clone(), set);
    }
    Ok(enums)
}

/// Formats are compared with Python's `re.fullmatch`, so each is wrapped in `\A(?:…)\z` rather
/// than trusted to carry its own anchors.
fn parse_formats(doc: &Value) -> Result<BTreeMap<String, Regex>, String> {
    let mut formats = BTreeMap::new();
    let Some(object) = doc.get("formats").and_then(Value::as_object) else {
        return Ok(formats);
    };
    for (name, pattern) in object {
        let source = pattern
            .as_str()
            .ok_or_else(|| format!("gate format '{name}' is not a string"))?;
        let compiled = Regex::new(&format!(r"\A(?:{source})\z"))
            .map_err(|e| format!("gate format '{name}' is not a valid regex: {e}"))?;
        formats.insert(name.clone(), compiled);
    }
    Ok(formats)
}

fn parse_bounds(doc: &Value) -> Result<BTreeMap<String, (f64, f64)>, String> {
    let mut bounds = BTreeMap::new();
    let Some(object) = doc.get("numericBounds").and_then(Value::as_object) else {
        return Ok(bounds);
    };
    for (unit, pair) in object {
        let values = pair
            .as_array()
            .filter(|values| values.len() == 2)
            .ok_or_else(|| format!("gate numericBounds '{unit}' is not a two-element list"))?;
        let lo = values[0]
            .as_f64()
            .ok_or_else(|| format!("gate numericBounds '{unit}' has a non-numeric low bound"))?;
        let hi = values[1]
            .as_f64()
            .ok_or_else(|| format!("gate numericBounds '{unit}' has a non-numeric high bound"))?;
        bounds.insert(unit.clone(), (lo, hi));
    }
    Ok(bounds)
}

fn parse_exact_paths(doc: &Value) -> Result<BTreeMap<String, String>, String> {
    let mut exact = BTreeMap::new();
    for (format, key) in EXACT_PATH_LISTS {
        let Some(items) = doc.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let path = item
                .as_str()
                .ok_or_else(|| format!("gate '{key}' holds a non-string path"))?;
            exact.insert(path.to_string(), format.to_string());
        }
    }
    Ok(exact)
}

fn parse_patterns(doc: &Value) -> Result<Vec<(String, Pattern)>, String> {
    let mut patterns = Vec::new();
    let Some(items) = doc.get("forbiddenValuePatterns").and_then(Value::as_array) else {
        return Ok(patterns);
    };
    for item in items {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "gate forbiddenValuePatterns entry has no id".to_string())?;
        if id == EPOCH_PATTERN_ID {
            // The JSON keeps the Python lookaround regex; `epoch_in_string` is its equivalent.
            patterns.push((id.to_string(), Pattern::Fn(epoch_in_string)));
            continue;
        }
        if id == WHITESPACE_PATTERN_ID {
            // The JSON keeps `\s`; `python_whitespace` is what Python means by it.
            patterns.push((id.to_string(), Pattern::Fn(python_whitespace)));
            continue;
        }
        let source = item
            .get("regex")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("gate pattern '{id}' has no regex"))?;
        let compiled = Regex::new(source)
            .map_err(|e| format!("gate pattern '{id}' is not a valid regex: {e}"))?;
        patterns.push((id.to_string(), Pattern::Regex(compiled)));
    }
    Ok(patterns)
}

/// Python's `_typename`, which reports `boolean` before `integer` so a bool never reads as a count.
fn typename(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// JSON integers only: a bool is not a number here, and a float is not an integer.
fn as_integer(value: &Value) -> Option<i128> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(i128::from)),
        _ => None,
    }
}

fn is_calendar_day(value: &str) -> bool {
    // Python's `datetime.date.fromisoformat` rejects year 0 (`datetime.MINYEAR` is 1) while chrono's
    // year range includes it, so "0000-01-01" would otherwise pass here and fail there.
    NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok_and(|day| day.year() >= 1)
}

/// Join a parent path with a child key. The root is the empty string, so it gains no leading dot.
fn join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

/// One walk of one payload. Mirrors `check_field_gate._Checker`.
struct Checker<'a> {
    gate: &'a Gate,
    dynamic: &'a Dynamic,
    violations: Vec<String>,
}

impl Checker<'_> {
    fn fail(&mut self, path: &str, rule: &str, detail: &str) {
        let at = if path.is_empty() { "<root>" } else { path };
        self.violations.push(format!("{at}: {rule}: {detail}"));
    }

    fn walk(&mut self, value: &Value, path: &str, gpath: &str) {
        match value {
            Value::Object(map) => self.walk_dict(map, path, gpath),
            Value::Array(items) => self.walk_list(items, path, gpath),
            _ => self.check_leaf(value, path, gpath),
        }
    }

    fn walk_dict(&mut self, value: &Map<String, Value>, path: &str, gpath: &str) {
        let gate = self.gate;
        if !gpath.is_empty() && !gate.object_paths.contains(gpath) {
            match gate.fields.get(gpath) {
                None => self.fail(path, "unknown_key", "object is not a gate path"),
                Some(spec) => {
                    let detail = format!("expected {}, got object", spec.ty.name());
                    self.fail(path, "type_mismatch", &detail);
                }
            }
            return;
        }
        for (key, child) in value {
            self.walk_child(key, child, path, gpath);
        }
        if let Some(required) = gate.required_children.get(gpath) {
            for key in required {
                if !value.contains_key(key) {
                    let detail = format!("required key '{key}' is absent");
                    self.fail(path, "missing_required", &detail);
                }
            }
        }
    }

    fn walk_child(&mut self, key: &str, child: &Value, path: &str, gpath: &str) {
        let gate = self.gate;
        if !KEY_NAME_RE.is_match(key) {
            let detail = format!(
                "a key of length {} is not a plain identifier",
                key.chars().count()
            );
            self.fail(path, "bad_key_name", &detail);
            return;
        }
        let child_gpath = join(gpath, key);
        let element_gpath = format!("{child_gpath}[]");
        let known = gate.fields.contains_key(&child_gpath)
            || gate.object_paths.contains(&child_gpath)
            || gate.fields.contains_key(&element_gpath)
            || gate.object_paths.contains(&element_gpath);
        if !known {
            // Never echo the key: an unknown key could itself be the content the gate withholds.
            let detail = format!("a key of length {} is not a gate path", key.chars().count());
            self.fail(path, "unknown_key", &detail);
            return;
        }
        self.walk(child, &join(path, key), &child_gpath);
    }

    fn walk_list(&mut self, value: &[Value], path: &str, gpath: &str) {
        let gate = self.gate;
        let element_gpath = format!("{gpath}[]");
        let element_known =
            gate.fields.contains_key(&element_gpath) || gate.object_paths.contains(&element_gpath);
        if !element_known {
            let rule = if gate.fields.contains_key(gpath) || gate.object_paths.contains(gpath) {
                "type_mismatch"
            } else {
                "unknown_key"
            };
            self.fail(path, rule, "array is not a gate path");
            return;
        }
        for (index, item) in value.iter().enumerate() {
            self.walk(item, &format!("{path}[{index}]"), &element_gpath);
        }
    }

    fn check_leaf(&mut self, value: &Value, path: &str, gpath: &str) {
        let gate = self.gate;
        let Some(spec) = gate.fields.get(gpath) else {
            if gate.object_paths.contains(gpath) {
                let detail = format!("expected object, got {}", typename(value));
                self.fail(path, "type_mismatch", &detail);
            } else {
                self.fail(path, "unknown_key", "leaf is not a gate path");
            }
            return;
        };
        if value.is_null() {
            if !spec.nullable {
                self.fail(path, "null_not_allowed", "field is not nullable");
            }
            return;
        }
        match spec.ty {
            FieldType::Boolean => {
                if !value.is_boolean() {
                    self.mismatch(path, "boolean", value);
                }
            }
            FieldType::Integer => match as_integer(value) {
                Some(number) => self.check_number(number, path, spec),
                None => self.mismatch(path, "integer", value),
            },
            FieldType::String => match value.as_str() {
                Some(text) => self.check_string(text, path, gpath, spec),
                None => self.mismatch(path, "string", value),
            },
            FieldType::Object => self.mismatch(path, "object", value),
        }
    }

    fn mismatch(&mut self, path: &str, expected: &str, value: &Value) {
        let detail = format!("expected {expected}, got {}", typename(value));
        self.fail(path, "type_mismatch", &detail);
    }

    fn check_number(&mut self, value: i128, path: &str, spec: &FieldSpec) {
        let gate = self.gate;
        let unit = spec.unit.as_deref();
        let as_float = value as f64;
        if let Some(unit) = unit {
            // `validate_references` guarantees the entry exists.
            if let Some(&(lo, hi)) = gate.bounds.get(unit) {
                if !(lo..=hi).contains(&as_float) {
                    let detail = format!("{value} is outside {unit} bounds [{lo}, {hi}]");
                    self.fail(path, "out_of_bounds", &detail);
                }
            }
        }
        if EPOCH_RANGES
            .iter()
            .any(|&(lo, hi)| (lo..=hi).contains(&as_float))
        {
            let detail = "integer is in a unix-timestamp range";
            self.fail(path, "epoch_in_number", detail);
        }
    }

    fn check_string(&mut self, value: &str, path: &str, gpath: &str, spec: &FieldSpec) {
        let gate = self.gate;
        if let Some(exact) = gate.exact_paths.get(gpath) {
            // Exact-format paths are not free text, so pattern and dynamic rules do not apply.
            if !gate.formats[exact].is_match(value) {
                let length = value.chars().count();
                let detail = format!("expected exact {exact}, got string of length {length}");
                self.fail(path, "format_mismatch", &detail);
            } else if exact == "day" && !is_calendar_day(value) {
                self.fail(path, "format_mismatch", "day is not a calendar date");
            }
            return;
        }
        if let Some(name) = &spec.enum_name {
            if !gate.enums[name].contains(value) {
                let length = value.chars().count();
                let detail = format!("string of length {length} is not in enum {name}");
                self.fail(path, "not_in_enum", &detail);
                return;
            }
            // A value drawn from a closed enum cannot carry a local-only string: it is one of the
            // gate's own literals. The dynamic substring rule would otherwise misfire whenever a
            // short asset name (a skill called "run") happens to sit inside an enum value
            // ("truncated"). Patterns still run, so an enum literal can never itself be a path.
            self.check_patterns(value, path);
            return;
        }
        if let Some(name) = &spec.format {
            if !gate.formats[name].is_match(value) {
                let length = value.chars().count();
                let detail = format!("string of length {length} does not match {name}");
                self.fail(path, "format_mismatch", &detail);
            }
        }
        self.check_patterns(value, path);
        self.check_dynamic(value, path);
    }

    fn check_patterns(&mut self, value: &str, path: &str) {
        let gate = self.gate;
        for (id, pattern) in &gate.patterns {
            if pattern.is_match(value) {
                let rule = format!("pattern:{id}");
                let detail = format!("matched forbidden pattern {id}");
                self.fail(path, &rule, &detail);
            }
        }
    }

    fn check_dynamic(&mut self, value: &str, path: &str) {
        let dynamic = self.dynamic;
        let lowered = value.to_lowercase();
        for (name, needles) in &dynamic.sets {
            if needles
                .iter()
                .any(|needle| lowered.contains(needle.as_str()))
            {
                let rule = format!("dynamic:{name}");
                let detail = format!("contains an entry of local-only set {name}");
                self.fail(path, &rule, &detail);
            }
        }
    }
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
