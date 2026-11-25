//! Unit tests for the fixture, identity rules and view rows.

use super::components::{apply, resolve, Resolution};
use super::*;
use crate::data::Data;

mod corpus;
mod stress;

fn rows(view: View, rt: &Runtime) -> Vec<Row> {
    content(&Page::new(view), &Data::fixture(), false, 80, rt).2
}

fn texts(view: View, rt: &Runtime) -> Vec<String> {
    rows(view, rt).into_iter().map(|r| r.text).collect()
}

fn opens_placement(rows: &[Row], pid: usize) -> bool {
    rows.iter()
        .any(|r| matches!(r.open, Some((View::Placement(q), _)) if q == pid))
}

fn find_use(day: u32, ws: &str, component: usize) -> (usize, usize) {
    let all = Data::fixture();
    let sid = all
        .iter()
        .position(|s| s.day == day && s.workspace == ws)
        .unwrap();
    let u = all[sid]
        .observed
        .iter()
        .position(|u| u.component == component)
        .unwrap();
    (sid, u)
}

#[test]
fn spike_reconciles_and_subagent_is_counted_once() {
    let sessions = Data::fixture();
    let day: Vec<_> = sessions.iter().filter(|s| s.day == 16).collect();
    assert_eq!(day.len(), 2);
    assert_eq!(day.iter().map(|s| s.cents()).sum::<u32>(), 852);
    assert_eq!(day[0].cents(), 820);
}

#[test]
fn unpriced_session_keeps_tokens_without_zero_cost_bars() {
    let all = Data::fixture();
    let i = all.iter().position(|s| s.incomplete).unwrap();
    let rt = Runtime::default();
    let (_, scope, rows) = content(&Page::new(View::Session(i)), &all, false, 80, &rt);
    assert!(scope.contains("No priced requests"));
    assert!(rows.iter().all(|r| !r.text.contains("$0.00")));
    assert!(texts(View::Tokens(i), &rt)
        .iter()
        .any(|t| t.contains("30000 tokens")));
}

// Two workspaces both install review-kit 1.5. Only the api artifact was
// scanned, so the web copy must not borrow its grade by version label.
#[test]
fn same_version_label_never_confers_scan_or_current_copy() {
    let rt = Runtime::default();
    assert!(texts(View::Placement(0), &rt)
        .iter()
        .any(|t| t.contains("RK-15")));
    assert!(texts(View::Placement(2), &rt)
        .iter()
        .any(|t| t == "Scan      None for this artifact"));
    let (sid, u) = find_use(16, "web", 0);
    let web = rows(View::Observation(sid, u), &rt);
    assert!(opens_placement(&web, 2));
    assert!(!opens_placement(&web, 0));
}

// A historical 1.4 observation leads to the user copy with that artifact,
// even though a different copy is effective in the workspace today.
#[test]
fn observation_links_to_current_copy_only_by_artifact() {
    let rt = Runtime::default();
    let (sid, u) = find_use(16, "api", 0);
    let observed = rows(View::Observation(sid, u), &rt);
    assert!(opens_placement(&observed, 1));
    assert!(!opens_placement(&observed, 0));
    let (sid, u) = find_use(12, "api", 0);
    let unknown = texts(View::Observation(sid, u), &rt);
    assert!(unknown.contains(&"Installed unknown: artifact not recorded".to_string()));
    assert!(unknown.contains(&"Scan      Unavailable: no artifact".to_string()));
}

#[test]
fn disabling_api_copy_falls_back_without_touching_web_or_history() {
    let mut rt = Runtime::default();
    let history = |rt: &Runtime| -> Vec<String> {
        texts(View::Component(0, Scope::WEEK), rt)
            .into_iter()
            .filter(|t| t.starts_with("Nov"))
            .collect()
    };
    let before = history(&rt);
    assert_eq!(
        resolve(0, "api", "Claude", &rt.disabled),
        Resolution::Copy(0)
    );
    apply(0, true, &mut rt);
    assert_eq!(
        resolve(0, "api", "Claude", &rt.disabled),
        Resolution::Copy(1)
    );
    assert_eq!(
        resolve(0, "web", "Claude", &rt.disabled),
        Resolution::Copy(1)
    );
    assert!(matches!(
        resolve(0, "web", "Codex", &rt.disabled),
        Resolution::Unknown(_)
    ));
    assert_eq!(before, history(&rt));
    apply(0, false, &mut rt);
    assert!(rt.disabled.is_empty());
    assert_eq!(
        resolve(0, "api", "Claude", &rt.disabled),
        Resolution::Copy(0)
    );
}

#[test]
fn change_preview_predicts_fallback_or_states_unknown() {
    let rt = Runtime::default();
    assert!(
        texts(View::Change(0, true), &rt).contains(&"Predicted user 1.4 (sha:1c4e)".to_string())
    );
    assert!(texts(View::Change(2, true), &rt)
        .iter()
        .any(|t| t.starts_with("Predicted Unknown")));
}

#[test]
fn session_scope_stays_on_its_day_and_workspace_until_widened() {
    let all = Data::fixture();
    let rt = Runtime::default();
    let (sid, _) = find_use(16, "api", 0);
    let day = View::Component(0, Scope::of(&all[sid]));
    let observed = |view: View| -> Vec<String> {
        texts(view, &rt)
            .into_iter()
            .filter(|t| t.starts_with("Nov"))
            .collect()
    };
    assert_eq!(observed(day), ["Nov 16 api Index rebuild  1.4   2 calls"]);
    let week = observed(day.widened().unwrap());
    assert_eq!(week.len(), 6);
    assert!(week.iter().all(|t| t.contains(" api ")));
}

// Beside the preview pane at 80 columns a row has 45 cells. A maintainer
// triaging the list must see every row's state, unknown resolution first.
#[test]
fn inventory_state_fits_row_and_surfaces_unknown() {
    let mut rt = Runtime::default();
    apply(0, true, &mut rt);
    let rows = texts(View::Inventory, &rt);
    assert!(rows.iter().all(|t| t.chars().count() <= 45), "{rows:?}");
    let review = rows.iter().find(|t| t.contains("review-kit")).unwrap();
    assert!(review.ends_with(" Unknown +2 more"), "{review}");
}

#[test]
fn empty_inventory_filter_reports_zero_results() {
    let mut page = Page::new(View::Inventory);
    page.filters = [4, 0, 2, 0];
    let (_, scope, rows) = content(&page, &Data::fixture(), false, 80, &Runtime::default());
    assert!(rows.is_empty());
    assert!(scope.starts_with("0 of 10 |"));
}

mod render;
