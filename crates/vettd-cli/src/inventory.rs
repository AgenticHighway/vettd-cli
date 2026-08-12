//! Inventory command implementations.
//!
//! Mirrors `directory.rs`'s command shape, but scoped to the authenticated
//! user's own assets (published and unpublished) rather than the public
//! skill registry. All requests go through `crate::inventory_client`, which
//! always attaches an `Authorization` header — never `read_client`.
//!
//! Response shapes are identical to the directory API, so this module reuses
//! `directory`'s response structs and display helpers rather than duplicating
//! them.

use crate::directory::{self, DirectoryFinding, DirectoryListResponse, DirectorySkillDetail};
use crate::inventory_client::{self, InventoryError};

/// Derive the inventory API base URL from the configured ingest endpoint.
///
/// `VETTD_INVENTORY_ENDPOINT` overrides the ingest endpoint used for
/// derivation, for pointing inventory search at a test/staging API. Only
/// honored when `SEARCH_BETA_TESTING` is enabled (see
/// [`crate::network::search_beta_testing_enabled`]).
fn inventory_base_url() -> String {
    let override_endpoint = crate::network::search_beta_testing_enabled()
        .then(|| std::env::var("VETTD_INVENTORY_ENDPOINT").ok())
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let endpoint = override_endpoint
        .or_else(|| crate::submit::load_auth_config().map(|c| c.endpoint))
        .unwrap_or_else(|| crate::submit::DEFAULT_PRODUCTION_ENDPOINT.to_string());
    crate::network::derive_api_url(&endpoint, "inventory")
}

/// Print the standard "run `vettd auth` first" message and exit with code 3.
///
/// Called up front by every inventory subcommand handler before any network
/// request is attempted.
pub fn require_auth_or_exit() {
    if !inventory_client::is_authenticated() {
        eprintln!("Error: not authenticated — run `vettd auth` to configure your API key.");
        std::process::exit(3);
    }
}

/// Fetch a single skill detail from the user's inventory, mapping errors to
/// clear exit messages.
fn fetch_skill(slug: &str) -> DirectorySkillDetail {
    let base = inventory_base_url();
    let url = format!("{base}/{}", directory::percent_encode(slug));
    match inventory_client::fetch_json::<DirectorySkillDetail>(&url) {
        Ok(detail) => detail,
        Err(InventoryError::Unauthenticated) => {
            eprintln!("Error: not authenticated — run `vettd auth` to configure your API key.");
            std::process::exit(3);
        }
        Err(InventoryError::NotFound) => {
            eprintln!("Error: skill '{slug}' not found in your inventory.");
            std::process::exit(1);
        }
        Err(InventoryError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd inventory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error fetching skill '{slug}': {e}");
            std::process::exit(1);
        }
    }
}

/// Map a letter grade (A/B/C/F — the only grades the real data model uses)
/// onto the same 0-4 scale as `directory::severity_value`, so `--min-rating`
/// can reuse severity filtering/comparison logic. `A` is the safest grade
/// (lowest bar, matches "info"); `F` is the most severe (matches "critical")
/// — this mirrors the color convention already used by `directory::grade_color`.
fn rating_value(grade: &str) -> u8 {
    match grade.to_ascii_uppercase().as_str() {
        "F" => 4,
        "C" => 2,
        "B" => 1,
        _ => 0, // "A" and anything unrecognized
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

pub fn handle_list(page: u32, sort: &str, reverse: bool, json: bool) {
    require_auth_or_exit();
    let url = format!(
        "{}?{}&page={page}",
        inventory_base_url(),
        directory::api_sort_params(sort, reverse)
    );
    match inventory_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                directory::print_cards(&resp.skills, false);
                let shown = resp.skills.len();
                if resp.page < resp.total_pages {
                    println!(
                        "\nShowing {} of {} assets — use --page {} to see more.",
                        shown,
                        resp.total,
                        resp.page + 1,
                    );
                } else {
                    println!("\nShowing {} of {} assets.", shown, resp.total);
                }
            }
        }
        Err(InventoryError::Unauthenticated) => {
            eprintln!("Error: not authenticated — run `vettd auth` to configure your API key.");
            std::process::exit(3);
        }
        Err(InventoryError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd inventory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn handle_search(
    query: &str,
    page: u32,
    sort: &str,
    reverse: bool,
    json: bool,
    languages: &[String],
    agent_compatibility: &[String],
    rankings: Option<&str>,
) {
    let beta = crate::network::search_beta_testing_enabled();
    let rankings_value =
        directory::validate_search_filters(languages, agent_compatibility, rankings, beta);
    require_auth_or_exit();

    let result = if beta {
        let body = directory::build_search_body(
            query,
            page,
            sort,
            reverse,
            languages,
            agent_compatibility,
            rankings_value,
        );
        inventory_client::post_json::<DirectoryListResponse>(&inventory_base_url(), &body)
    } else {
        let url = format!(
            "{}?search={}&{}&page={page}",
            inventory_base_url(),
            directory::percent_encode(query),
            directory::api_sort_params(sort, reverse),
        );
        inventory_client::fetch_json::<DirectoryListResponse>(&url)
    };

    match result {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else if resp.skills.is_empty() {
                println!("No results for \"{}\" in your inventory.", query);
            } else {
                directory::print_cards(&resp.skills, false);
                let shown = resp.skills.len();
                if resp.page < resp.total_pages {
                    println!(
                        "\nShowing {} of {} assets for \"{}\" — use --page {} to see more.",
                        shown,
                        resp.total,
                        query,
                        resp.page + 1,
                    );
                } else {
                    println!(
                        "\nShowing {} of {} assets for \"{}\".",
                        shown, resp.total, query,
                    );
                }
            }
        }
        Err(InventoryError::Unauthenticated) => {
            eprintln!("Error: not authenticated — run `vettd auth` to configure your API key.");
            std::process::exit(3);
        }
        Err(InventoryError::Unreachable(msg)) => {
            eprintln!("Error: could not reach the vettd inventory: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub fn handle_view(slug: &str, json: bool) {
    require_auth_or_exit();
    let detail = fetch_skill(slug);
    if json {
        let mut val = serde_json::to_value(&detail).unwrap_or_default();
        if let Some(obj) = val.as_object_mut() {
            obj.remove("findings");
        }
        println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
        return;
    }

    println!("{}", detail.name);
    if let Some(desc) = &detail.description {
        println!("  {desc}");
    }
    println!();
    println!(
        "  {:<13}  {}",
        "Grade:",
        detail.overall_grade.as_deref().unwrap_or("—")
    );
    println!(
        "  {:<13}  {}",
        "Version:",
        detail.version.as_deref().unwrap_or("—")
    );
    println!(
        "  {:<13}  {}",
        "License:",
        detail.license.as_deref().unwrap_or("—")
    );
    println!(
        "  {:<13}  {}",
        "Category:",
        detail.category.as_deref().unwrap_or("—")
    );
    println!(
        "  {:<13}  {}",
        "Files:",
        detail
            .file_count
            .map(|n| n.to_string())
            .as_deref()
            .unwrap_or("—")
    );
    println!();
    let display_slug = detail.slug.as_deref().unwrap_or(slug);
    println!("  Run `vettd inventory findings {display_slug}` to see finding details.");
}

pub fn handle_findings(slug: &str, min_rating: &str, json: bool) {
    require_auth_or_exit();
    let detail = fetch_skill(slug);
    let threshold = rating_value(min_rating);
    let filtered: Vec<&DirectoryFinding> = detail
        .findings
        .iter()
        .filter(|f| directory::severity_value(&f.severity) >= threshold)
        .collect();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&filtered).unwrap_or_default()
        );
        return;
    }

    let total = detail.findings.len();
    let shown = filtered.len();
    let rating_display = min_rating.to_ascii_uppercase();
    println!(
        "Findings for {}  (--min-rating {rating_display})",
        detail.name
    );
    println!();

    if filtered.is_empty() {
        println!("  No findings at or above the '{rating_display}' rating threshold.");
    } else {
        for f in &filtered {
            let rule = f.rule_id.as_deref().unwrap_or("—");
            println!("  [{}]  {}  ({rule})", f.severity.to_uppercase(), f.label);
            if let Some(detail_text) = &f.detail {
                println!("       {detail_text}");
            }
            println!();
        }
        println!("  Showing {shown}/{total} findings (filter: >= {rating_display} rating).");
    }
}

pub fn handle_compare(slug_a: &str, slug_b: &str, json: bool) {
    require_auth_or_exit();
    let detail_a = fetch_skill(slug_a);
    let detail_b = fetch_skill(slug_b);

    if json {
        #[derive(serde::Serialize)]
        struct CompareOutput<'a> {
            a: &'a DirectorySkillDetail,
            b: &'a DirectorySkillDetail,
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&CompareOutput {
                a: &detail_a,
                b: &detail_b,
            })
            .unwrap_or_default()
        );
        return;
    }

    // Column geometry mirrors directory::handle_compare's approach: truncate
    // each value to fit its column so long values can't run into each other,
    // and separate the header from the data with a rule.
    let label_w: usize = 10;
    let val_w: usize = 30;
    let col = |s: &str| directory::truncate_to_display(s, val_w);

    let slug_display_a = detail_a.slug.as_deref().unwrap_or(slug_a);
    let slug_display_b = detail_b.slug.as_deref().unwrap_or(slug_b);
    let files_a = detail_a
        .file_count
        .map_or_else(|| "—".to_string(), |n| n.to_string());
    let files_b = detail_b
        .file_count
        .map_or_else(|| "—".to_string(), |n| n.to_string());

    println!(
        "{:label_w$}{:<val_w$}  {}",
        "",
        col(slug_display_a),
        col(slug_display_b)
    );
    println!("{}", "─".repeat(label_w + val_w + 2 + val_w));
    println!(
        "{:<label_w$}{:<val_w$}  {}",
        "Grade:",
        col(detail_a.overall_grade.as_deref().unwrap_or("—")),
        col(detail_b.overall_grade.as_deref().unwrap_or("—"))
    );
    println!(
        "{:<label_w$}{:<val_w$}  {}",
        "Files:",
        col(&files_a),
        col(&files_b)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rating_value_matches_severity_scale() {
        assert_eq!(rating_value("A"), 0);
        assert_eq!(rating_value("B"), 1);
        assert_eq!(rating_value("C"), 2);
        assert_eq!(rating_value("F"), 4);
        assert_eq!(rating_value("a"), 0, "case-insensitive");
        assert_eq!(
            rating_value("nonsense"),
            0,
            "unrecognized defaults to lowest bar"
        );
    }
}
