//! History's data on the fixture: the corpus's own sessions, held apart
//! from the dashboard's.

use super::*;

// The order must be the same on every run, so a person can find a row
// again: newest first, then agent, then name.
#[test]
fn stress_history_lists_every_session_newest_first() {
    let all = Data::stress();
    let past = all.corpus_history();
    assert_eq!(past.len(), 1991);
    assert_eq!(past.len(), all.len());
    assert_eq!(past[0].day, 18);
    for pair in past.windows(2) {
        let key = |s: &Session| (std::cmp::Reverse(s.day), s.agent, s.name);
        assert!(
            key(&pair[0]) <= key(&pair[1]),
            "{} before {}",
            pair[0].name,
            pair[1].name
        );
    }
}

// History lives in its own `Data`; the dashboard's figures and plain
// export must come out byte for byte as before it was loaded.
#[test]
fn dashboard_data_and_export_are_unchanged_by_history() {
    for (all, corpus) in [(Data::fixture(), "default"), (Data::stress(), "stress")] {
        let before = crate::export(&all, corpus).to_string();
        let sessions = all.len();
        let past = all.corpus_history();
        assert_eq!(past.len(), sessions);
        assert_eq!(all.len(), sessions);
        assert_eq!(crate::export(&all, corpus).to_string(), before);
    }
}

// `--history` is only an export option: without `--json` it would do
// nothing, so it is refused, as the `--corpus` mistakes are.
#[test]
fn history_flag_needs_json() {
    let error = |args: &[&str]| parse_args(args.iter().map(|s| s.to_string())).error;
    assert_eq!(error(&["--history"]), Some(HISTORY_ERROR.to_string()));
    assert_eq!(
        error(&["--source", "local", "--history"]),
        Some(HISTORY_ERROR.to_string())
    );
    assert_eq!(error(&["--json", "--history"]), None);
    assert_eq!(error(&["--history", "--json"]), None);
    assert!(!parse_args(["--json".to_string()]).history);
}
