//! The disclosure `vettd observe` shows before it reads anything.
//!
//! `vettd observe` reads local harness session logs, so the user is told what an observation
//! includes *before any session file is opened* — on every path, including the ones that send
//! nothing (`--dry-run`) and the one that refuses because telemetry is off.
//!
//! Two properties are what make that honest. The wording is not authored here: every line is a
//! [`DisclosureCategory`] label and description, and those are copied verbatim from the repo-root
//! `telemetry-field-gate.json` that admits the fields in the first place — `contract::disclosure`'s
//! `every_gate_category_is_a_disclosure_variant` proves the two lists agree byte for byte, so the
//! text cannot drift from the egress allowlist. And the categories are listed *structurally* — all
//! fourteen, always — rather than derived from a built envelope: the disclosure precedes the
//! reading, so it has to name the whole surface an observation may carry, not the subset one
//! particular run happened to produce.
//!
//! The shape mirrors [`crate::contract::render_disclosure`]: this returns the text and the caller
//! picks the stream. Callers write it to stderr; machine-readable output owns stdout.

use std::path::Path;

use crate::contract::DisclosureCategory;

/// The telemetry categories, in `telemetry-field-gate.json` `disclosureCategories` order.
///
/// Fixed and complete rather than payload-derived — see the module docs for why the disclosure
/// names the full surface instead of one run's subset.
pub(crate) const TELEMETRY_CATEGORIES: [DisclosureCategory; 14] = [
    DisclosureCategory::TelemetryBookkeeping,
    DisclosureCategory::ObservationDay,
    DisclosureCategory::DeviceIdentity,
    DisclosureCategory::HarnessIdentity,
    DisclosureCategory::ModelIdentity,
    DisclosureCategory::RunShape,
    DisclosureCategory::RunOutcomeCounts,
    DisclosureCategory::RunTokenTotals,
    DisclosureCategory::AssetIdentityHash,
    DisclosureCategory::AssetLoadedSet,
    DisclosureCategory::AssetOutcomeCounts,
    DisclosureCategory::AssetTimingStats,
    DisclosureCategory::AssetTokenStats,
    DisclosureCategory::CoverageMetadata,
];

/// Format the `vettd observe` disclosure.
///
/// `root` is the harness home whose `projects` directory will be read (`~/.claude` by default);
/// `destination` is the host an envelope would be submitted to, or `None` when nothing will be
/// sent. The result is a header line, one `    • {label} — {description}` line per category, the
/// source line, an optional destination line, and a final newline — exactly the layout
/// [`crate::contract::render_disclosure`] produces, so `eprint!` renders it without adding or
/// swallowing a line break.
///
/// Performs no I/O: `root` is rendered as given and never touched on disk, so printing the
/// disclosure can never be the read it is announcing.
pub(crate) fn render_observe_disclosure(destination: Option<&str>, root: &Path) -> String {
    let mut lines = Vec::with_capacity(4 + TELEMETRY_CATEGORIES.len());
    lines.push("  This observation will include:".to_string());

    for cat in TELEMETRY_CATEGORIES {
        lines.push(format!("    • {} — {}", cat.label(), cat.description()));
    }

    lines.push(format!(
        "  Source: Claude Code session logs under {}/projects (read-only; message text, paths, \
         names and ids never leave this machine)",
        root.display()
    ));

    if let Some(host) = destination {
        lines.push(format!("  Destination: {host}"));
    }

    lines.push(String::new()); // trailing blank line
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const ROOT: &str = "/home/example/.claude";
    const GATE_JSON: &str = include_str!("../../../../telemetry-field-gate.json");

    /// `TELEMETRY_CATEGORIES` is the disclosure's own list, so checking the rendered bullets
    /// against it proves only that rendering works. This anchors the list to the gate file itself:
    /// a category added to `disclosureCategories` that nobody adds here would otherwise ship
    /// fields under a heading the user was never shown, and every test would still pass. Verified
    /// by adding a fifteenth gate category during review: nothing failed until this test existed.
    #[test]
    fn the_category_list_is_the_gate_s_list_in_the_gate_s_order() {
        let doc: Value = serde_json::from_str(GATE_JSON).expect("gate JSON parses");
        let gate_names: Vec<&str> = doc["disclosureCategories"]
            .as_array()
            .expect("disclosureCategories is a list")
            .iter()
            .map(|entry| entry["name"].as_str().expect("each category has a name"))
            .collect();
        let ours: Vec<&str> = TELEMETRY_CATEGORIES.iter().map(|c| c.name()).collect();
        assert_eq!(
            ours, gate_names,
            "the observe disclosure must name exactly the gate's categories, in its order"
        );
    }

    /// Nothing may egress under a heading the user was not shown, so the disclosure lists every
    /// category the field gate can admit — all fourteen, in the gate's order, in the gate's own
    /// words. Rendering fewer (or paraphrasing one) would let a run send fields the user never
    /// saw described.
    #[test]
    fn every_telemetry_category_is_listed_in_gate_order_with_gate_wording() {
        let text = render_observe_disclosure(None, Path::new(ROOT));
        let bullets: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("    • "))
            .collect();

        assert_eq!(
            bullets.len(),
            TELEMETRY_CATEGORIES.len(),
            "the disclosure must name every telemetry category:\n{text}"
        );
        for (line, cat) in bullets.iter().zip(TELEMETRY_CATEGORIES) {
            assert_eq!(
                *line,
                format!("    • {} — {}", cat.label(), cat.description()),
                "{} must be rendered with its gate label and description",
                cat.name()
            );
        }
    }

    /// The layout is a contract with the caller: the text is complete and self-terminating, so a
    /// caller `eprint!`s it as-is. A missing or doubled trailing newline, or a destination line
    /// that appears when nothing will be sent, would either corrupt the surrounding output or
    /// tell the user their data is going somewhere it is not.
    #[test]
    fn the_line_structure_is_fixed_and_ends_in_a_single_newline() {
        let local = render_observe_disclosure(None, Path::new(ROOT));
        let parts: Vec<&str> = local.split('\n').collect();

        // Header + 14 bullets + source line, then the terminator of that last line.
        assert_eq!(parts.len(), 17, "unexpected disclosure layout:\n{local}");
        assert_eq!(parts[0], "  This observation will include:");
        assert_eq!(
            parts[15],
            "  Source: Claude Code session logs under /home/example/.claude/projects (read-only; \
             message text, paths, names and ids never leave this machine)"
        );
        assert_eq!(parts[16], "", "the text ends with the last line's newline");
        assert!(
            local.ends_with('\n') && !local.ends_with("\n\n"),
            "exactly one trailing newline, as `render_disclosure` produces:\n{local:?}"
        );
        assert!(
            !local.contains("Destination:"),
            "no destination may be named when nothing will be sent:\n{local}"
        );

        let submitted = render_observe_disclosure(Some("app.vettd.ai"), Path::new(ROOT));
        let parts: Vec<&str> = submitted.split('\n').collect();
        assert_eq!(parts.len(), 18, "a destination adds one line:\n{submitted}");
        assert_eq!(parts[16], "  Destination: app.vettd.ai");
        assert_eq!(parts[17], "");
    }

    /// The disclosure is shown *before* any session file is opened, so producing it must not read
    /// the filesystem — a renderer that stat-ed or canonicalised `root` would make the announcement
    /// itself the first act of observation. It also writes nothing: the whole text comes back as a
    /// value, leaving the stream choice (stderr) to the caller, which is what keeps stdout usable
    /// for `--json`. The process-level check that stdout stays empty needs a binary to run, so it
    /// arrives with the command itself in Phase 6 of `docs/vettd-observe-port-plan.md`, as
    /// `observe_disclosure_rendering_does_not_write_to_stdout`; it does not exist yet.
    #[test]
    fn rendering_reads_no_file_and_emits_nothing_itself() {
        let missing = Path::new("/vettd-observe-disclosure-does-not-exist-8a41");
        assert!(!missing.exists(), "purity is only proven if root is absent");

        let first = render_observe_disclosure(Some("app.vettd.ai"), missing);
        let second = render_observe_disclosure(Some("app.vettd.ai"), missing);

        assert_eq!(
            first, second,
            "the renderer is a pure function of its inputs"
        );
        assert!(
            first.contains("/vettd-observe-disclosure-does-not-exist-8a41/projects"),
            "root is rendered verbatim, never resolved on disk:\n{first}"
        );
        assert!(
            first.starts_with("  This observation will include:\n    • "),
            "the returned value carries the disclosure from its first line:\n{first}"
        );
        assert!(
            first.ends_with("  Destination: app.vettd.ai\n"),
            "the returned value carries it through its last line too:\n{first}"
        );
    }
}
