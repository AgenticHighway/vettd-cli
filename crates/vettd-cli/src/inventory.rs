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
fn inventory_base_url() -> String {
    let endpoint = crate::submit::load_auth_config()
        .map(|c| c.endpoint)
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
    let url = format!("{base}/{}", percent_encode(slug));
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

// Re-export percent_encode's behavior locally (directory's is module-private
// by design — inventory builds its own tiny copy to avoid widening directory's
// public surface for a one-line helper).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn api_sort_params(sort: &str, reverse: bool) -> String {
    let s = match sort {
        "rating" => "verdict",
        other => other,
    };
    let default_asc = sort == "alpha";
    let dir = if default_asc ^ reverse { "asc" } else { "desc" };
    format!("sort={s}&dir={dir}")
}

/// Map a letter grade (A/B/C/D/F) onto the same 0-4 scale as
/// `directory::severity_value`, so `--min-rating` can reuse severity
/// filtering/comparison logic. `A` is the safest grade (lowest bar,
/// matches "info"); `F` is the most severe (matches "critical") — this
/// mirrors the color convention already used by `directory::grade_color`.
fn rating_value(grade: &str) -> u8 {
    match grade.to_ascii_uppercase().as_str() {
        "F" => 4,
        "D" => 3,
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
        api_sort_params(sort, reverse)
    );
    match inventory_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else {
                directory::print_cards(&resp.skills);
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

pub fn handle_search(query: &str, page: u32, sort: &str, reverse: bool, json: bool) {
    require_auth_or_exit();
    let url = format!(
        "{}?search={}&{}&page={page}",
        inventory_base_url(),
        percent_encode(query),
        api_sort_params(sort, reverse),
    );
    match inventory_client::fetch_json::<DirectoryListResponse>(&url) {
        Ok(resp) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp).unwrap_or_default()
                );
            } else if resp.skills.is_empty() {
                println!("No results for \"{}\" in your inventory.", query);
            } else {
                directory::print_cards(&resp.skills);
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

    let slug_display_a = detail_a.slug.as_deref().unwrap_or(slug_a);
    let slug_display_b = detail_b.slug.as_deref().unwrap_or(slug_b);
    println!("{:<15}{}", slug_display_a, slug_display_b);
    println!(
        "{:<15}{}",
        detail_a.overall_grade.as_deref().unwrap_or("—"),
        detail_b.overall_grade.as_deref().unwrap_or("—")
    );
    println!(
        "{:<15}{}",
        detail_a
            .file_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string()),
        detail_b
            .file_count
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_basic() {
        assert_eq!(percent_encode("hello world"), "hello%20world");
    }

    #[test]
    fn api_sort_params_default_desc() {
        assert_eq!(api_sort_params("newest", false), "sort=newest&dir=desc");
    }

    #[test]
    fn rating_value_matches_severity_scale() {
        assert_eq!(rating_value("A"), 0);
        assert_eq!(rating_value("B"), 1);
        assert_eq!(rating_value("C"), 2);
        assert_eq!(rating_value("D"), 3);
        assert_eq!(rating_value("F"), 4);
        assert_eq!(rating_value("a"), 0, "case-insensitive");
        assert_eq!(
            rating_value("nonsense"),
            0,
            "unrecognized defaults to lowest bar"
        );
    }
}
