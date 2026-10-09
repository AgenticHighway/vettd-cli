"""Field gate checker for the passive-observer telemetry envelope (spike #828).

The one prototype module meant to survive into #965. It validates a written payload against
``../telemetry-field-gate.json``:

- every leaf path must be listed in the gate's ``fields`` (an unknown key or an unknown intermediate
  object fails, mirroring ``contract/disclosure.rs::validate_payload_coverage``);
- nullable objects may be null, otherwise their children are checked as leaves;
- enums are closed, formats and numeric bounds are enforced per field;
- ``hashPaths`` / ``dayPaths`` / ``allowedUuidPaths`` are checked by exact format only;
- every other string leaf must pass every ``forbiddenValuePatterns`` regex and every dynamic forbid
  set the emitter hands over (substring, case-insensitive);
- numeric leaves outside the ``ms2`` / ``tokens2`` units fail when they look like a unix timestamp.

A key-path walker cannot see a path, a URL, a uuid or a name inside a string; the value-level rules are
what make "logs never leave the machine" checkable on the payload rather than on the code.

Path syntax is the disclosure.rs one: dot-joined keys, array elements as ``[]``. Violation strings name
the concrete instance path (``records[0].assets[1].asset_id``) so a failing record can be found, and
never echo a string value, because the value may be exactly the local-only name the gate exists to
keep off the wire. Stdlib only; ``check()`` is pure.
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import sys
from typing import Any, Dict, List, Optional, Set, Tuple

DEFAULT_GATE_PATH = os.path.normpath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "telemetry-field-gate.json")
)

# Sums of squares legitimately reach epoch-sized magnitudes; every other numeric unit does not.
EPOCH_EXEMPT_UNITS = ("ms2", "tokens2")
EPOCH_RANGES = ((1.5e9, 2.5e9), (1.5e12, 2.5e12))
# Dynamic-forbid entries shorter than this are skipped: they would match inside almost any value.
DYNAMIC_MIN_LEN = 3
KEY_NAME_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def load_gate(path: Optional[str] = None) -> dict:
    with open(path or DEFAULT_GATE_PATH, "r", encoding="utf-8") as fh:
        return json.load(fh)


class _Gate:
    """Lookups precomputed from one gate document."""

    def __init__(self, gate: dict):
        self.fields: Dict[str, dict] = gate["fields"]
        self.enums: Dict[str, list] = gate.get("enums", {})
        self.formats = {name: re.compile(rx) for name, rx in gate.get("formats", {}).items()}
        self.bounds: Dict[str, list] = gate.get("numericBounds", {})
        self.exact_paths: Dict[str, str] = {}
        for fmt, key in (("hex64", "hashPaths"), ("day", "dayPaths"), ("uuid", "allowedUuidPaths")):
            for path in gate.get(key, []):
                self.exact_paths[path] = fmt
        self.patterns = [(p["id"], re.compile(p["regex"])) for p in gate.get("forbiddenValuePatterns", [])]
        # Intermediate object paths (every proper prefix of a field path) and, per object path, the
        # child keys that must be present when that object is written.
        self.object_paths: Set[str] = set()
        self.required_children: Dict[str, Set[str]] = {}
        for path, spec in self.fields.items():
            segs = path.split(".")
            for i in range(1, len(segs)):
                self.object_paths.add(".".join(segs[:i]))
            if spec.get("required", True):
                for i in range(len(segs)):
                    parent = ".".join(segs[:i])
                    self.required_children.setdefault(parent, set()).add(segs[i].removesuffix("[]"))


COMPONENT_SETS = ("cwd_and_branches", "slugs", "home_dir")
COMPONENT_MIN_LEN = 4
_COMPONENT_SPLIT = re.compile(r"[/\\:._-]+")


def _normalize_dynamic(dynamic: Optional[Dict[str, Any]]) -> Dict[str, Tuple[str, ...]]:
    """Lower-case the emitter's sets, dropping empties and entries shorter than DYNAMIC_MIN_LEN.
    Path-like sets are also split into their components (a branch leaf, a directory name, a slug
    word) so a value that carries only part of a path is still caught. A set that is not a list of
    strings is an error, never silently ignored."""
    out: Dict[str, Tuple[str, ...]] = {}
    for name, values in (dynamic or {}).items():
        if isinstance(values, (str, bytes)) or not isinstance(values, (list, tuple, set, frozenset)):
            raise ValueError(f"dynamic set {name!r} must be a list of strings")
        needles = set()
        for v in values:
            if not isinstance(v, str):
                raise ValueError(f"dynamic set {name!r} holds a non-string entry")
            if len(v) >= DYNAMIC_MIN_LEN:
                needles.add(v.lower())
            if name in COMPONENT_SETS:
                for part in _COMPONENT_SPLIT.split(v):
                    if len(part) >= COMPONENT_MIN_LEN:
                        needles.add(part.lower())
        if needles:
            out[str(name)] = tuple(sorted(needles))
    return out


def _typename(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, int):
        return "integer"
    if isinstance(value, float):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "array"
    if isinstance(value, dict):
        return "object"
    return type(value).__name__


class _Checker:
    def __init__(self, gate: _Gate, dynamic: Optional[Dict[str, Any]]):
        self.gate = gate
        self.dynamic = _normalize_dynamic(dynamic)
        self.violations: List[str] = []

    def fail(self, path: str, rule: str, detail: str) -> None:
        self.violations.append(f"{path or '<root>'}: {rule}: {detail}")

    def walk(self, value: Any, path: str, gpath: str) -> None:
        if isinstance(value, dict):
            self._walk_dict(value, path, gpath)
        elif isinstance(value, list):
            self._walk_list(value, path, gpath)
        else:
            self._check_leaf(value, path, gpath)

    def _walk_dict(self, value: dict, path: str, gpath: str) -> None:
        if gpath and gpath not in self.gate.object_paths:
            spec = self.gate.fields.get(gpath)
            if spec is None:
                self.fail(path, "unknown_key", "object is not a gate path")
            else:
                self.fail(path, "type_mismatch", f"expected {spec['type']}, got object")
            return
        for key, child in value.items():
            if not isinstance(key, str) or not KEY_NAME_RE.fullmatch(key):
                self.fail(path, "bad_key_name", f"a key of length {len(str(key))} is not a plain identifier")
                continue
            child_gpath = f"{gpath}.{key}" if gpath else str(key)
            if child_gpath not in self.gate.fields and child_gpath not in self.gate.object_paths \
                    and child_gpath + "[]" not in self.gate.fields and child_gpath + "[]" not in self.gate.object_paths:
                # Never echo the key: an unknown key could itself be the content the gate withholds.
                self.fail(path, "unknown_key", f"a key of length {len(key)} is not a gate path")
                continue
            self.walk(child, f"{path}.{key}" if path else str(key), child_gpath)
        for key in sorted(self.gate.required_children.get(gpath, ())):
            if key not in value:
                self.fail(path, "missing_required", f"required key {key!r} is absent")

    def _walk_list(self, value: list, path: str, gpath: str) -> None:
        egpath = gpath + "[]"
        if egpath not in self.gate.fields and egpath not in self.gate.object_paths:
            known = gpath in self.gate.fields or gpath in self.gate.object_paths
            self.fail(path, "type_mismatch" if known else "unknown_key", "array is not a gate path")
            return
        for i, item in enumerate(value):
            self.walk(item, f"{path}[{i}]", egpath)

    def _check_leaf(self, value: Any, path: str, gpath: str) -> None:
        spec = self.gate.fields.get(gpath)
        if spec is None:
            if gpath in self.gate.object_paths:
                self.fail(path, "type_mismatch", f"expected object, got {_typename(value)}")
            else:
                self.fail(path, "unknown_key", "leaf is not a gate path")
            return
        if value is None:
            if not spec.get("nullable", False):
                self.fail(path, "null_not_allowed", "field is not nullable")
            return
        expected = spec["type"]
        if expected == "boolean":
            if not isinstance(value, bool):
                self.fail(path, "type_mismatch", f"expected boolean, got {_typename(value)}")
        elif expected == "integer":
            if isinstance(value, bool) or not isinstance(value, int):
                self.fail(path, "type_mismatch", f"expected integer, got {_typename(value)}")
            else:
                self._check_number(value, path, spec)
        elif expected == "string":
            if not isinstance(value, str):
                self.fail(path, "type_mismatch", f"expected string, got {_typename(value)}")
            else:
                self._check_string(value, path, gpath, spec)
        elif expected == "object":
            self.fail(path, "type_mismatch", f"expected object, got {_typename(value)}")
        else:
            raise ValueError(f"gate field {gpath!r} has unsupported type {expected!r}")

    def _check_number(self, value: int, path: str, spec: dict) -> None:
        unit = spec.get("unit")
        if unit is not None:
            if unit not in self.gate.bounds:
                raise ValueError(f"gate unit {unit!r} has no numericBounds entry")
            lo, hi = self.gate.bounds[unit]
            if not lo <= value <= hi:
                self.fail(path, "out_of_bounds", f"{value} is outside {unit} bounds [{lo}, {hi}]")
        if unit not in EPOCH_EXEMPT_UNITS and any(lo <= value <= hi for lo, hi in EPOCH_RANGES):
            self.fail(path, "epoch_in_number", "integer is in a unix-timestamp range")

    def _check_string(self, value: str, path: str, gpath: str, spec: dict) -> None:
        exact = self.gate.exact_paths.get(gpath)
        if exact is not None:
            # Hash, day and allowed-uuid paths are exact-format; they are not free text, so the
            # pattern and dynamic rules (which would trip on any hex64 or uuid) do not apply.
            if not self.gate.formats[exact].fullmatch(value):
                self.fail(path, "format_mismatch", f"expected exact {exact}, got string of length {len(value)}")
            elif exact == "day" and not _is_calendar_day(value):
                self.fail(path, "format_mismatch", "day is not a calendar date")
            return
        if "enum" in spec:
            if spec["enum"] not in self.gate.enums:
                raise ValueError(f"gate enum {spec['enum']!r} is not defined")
            if value not in self.gate.enums[spec["enum"]]:
                self.fail(path, "not_in_enum", f"string of length {len(value)} is not in enum {spec['enum']}")
                return
            # A value drawn from a closed enum cannot carry a local-only string: it is one of the
            # gate's own literals. The dynamic substring rule would otherwise misfire whenever a
            # short asset name (a skill called "run") happens to sit inside an enum value
            # ("truncated"). Patterns still run, so an enum literal can never itself be a path.
            for pid, rx in self.gate.patterns:
                if rx.search(value):
                    self.fail(path, f"pattern:{pid}", f"matched forbidden pattern {pid}")
            return
        if "format" in spec:
            if spec["format"] not in self.gate.formats:
                raise ValueError(f"gate format {spec['format']!r} is not defined")
            if not self.gate.formats[spec["format"]].fullmatch(value):
                self.fail(path, "format_mismatch", f"string of length {len(value)} does not match {spec['format']}")
        for pid, rx in self.gate.patterns:
            if rx.search(value):
                self.fail(path, f"pattern:{pid}", f"matched forbidden pattern {pid}")
        lowered = value.lower()
        for name, needles in self.dynamic.items():
            if any(needle in lowered for needle in needles):
                self.fail(path, f"dynamic:{name}", f"contains an entry of local-only set {name}")


def check(payload: Any, gate: dict, dynamic: Optional[Dict[str, Any]] = None) -> List[str]:
    """Return every violation of `gate` in `payload` (empty list = pass).

    `dynamic` maps a set name (see the gate's `dynamicForbids.sets`) to the local-only strings the
    emitter saw while parsing; every set given is enforced, whether or not the gate lists its name.
    """
    checker = _Checker(_Gate(gate), dynamic)
    checker.walk(payload, "", "")
    return checker.violations


def _is_calendar_day(value: str) -> bool:
    try:
        datetime.date.fromisoformat(value)
        return True
    except ValueError:
        return False


def _reject_duplicate_keys(pairs):
    """json.loads keeps the last duplicate; a payload whose first copy of a key carried a leak would
    be validated on the second. Duplicates are therefore an error."""
    obj: Dict[str, Any] = {}
    for key, value in pairs:
        if key in obj:
            raise ValueError("duplicate key in JSON object")
        obj[key] = value
    return obj


def _load_json(path: str) -> Any:
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh, object_pairs_hook=_reject_duplicate_keys)


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Check a telemetry payload against the field gate.")
    parser.add_argument("payload", help="payload JSON file to check")
    parser.add_argument("--gate", default=DEFAULT_GATE_PATH, help="gate JSON (default: ../telemetry-field-gate.json)")
    parser.add_argument("--dynamic", default=None, help="JSON file {set_name: [local-only strings]} to forbid")
    args = parser.parse_args(argv)
    try:
        payload = _load_json(args.payload)
        gate = load_gate(args.gate)
        dynamic = _load_json(args.dynamic) if args.dynamic else None
        if dynamic is not None and not isinstance(dynamic, dict):
            raise ValueError("--dynamic file must be a JSON object of {set_name: [strings]}")
    except (OSError, ValueError, TypeError, KeyError) as exc:
        print(f"check_field_gate: {exc}", file=sys.stderr)
        return 2
    try:
        violations = check(payload, gate, dynamic)
    except (ValueError, TypeError, KeyError) as exc:  # a malformed gate or dynamic file, never a traceback
        print(f"check_field_gate: {exc}", file=sys.stderr)
        return 2
    for line in violations:
        print(line)
    print(f"check_field_gate: {len(violations)} violation(s) against gate v{gate.get('gateVersion')}", file=sys.stderr)
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
