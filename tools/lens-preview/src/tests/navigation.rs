//! Unit tests for the keys that jump to a place.

use super::*;

fn page_at(selected: usize, top: usize) -> Page {
    let mut page = Page::new(View::Home);
    page.selected = selected;
    page.top = top;
    page
}

// End selects the last action and scrolls to the list's end; Home returns
// to the first action and the top, so leading rows are visible again.
#[test]
fn jump_reaches_both_ends_of_a_list_with_actions() {
    let view = Viewport {
        actions: 5,
        focus: Some(8),
        rows: 20,
        visible: 6,
    };
    let mut page = page_at(2, 4);
    view.jump(&mut page, true);
    assert_eq!((page.selected, page.top, page.follow), (4, 14, true));
    view.jump(&mut page, false);
    assert_eq!((page.selected, page.top, page.follow), (0, 0, true));
}

// With nothing to select, the keys still scroll to each end.
#[test]
fn jump_scrolls_a_list_without_actions() {
    let view = Viewport {
        actions: 0,
        focus: None,
        rows: 30,
        visible: 10,
    };
    let mut page = page_at(0, 7);
    view.jump(&mut page, true);
    assert_eq!((page.selected, page.top, page.follow), (0, 20, false));
    view.jump(&mut page, false);
    assert_eq!((page.top, page.follow), (0, false));
}

// A list shorter than the screen has nowhere to scroll.
#[test]
fn jump_on_a_short_list_keeps_the_top() {
    let view = Viewport {
        actions: 3,
        focus: Some(1),
        rows: 4,
        visible: 10,
    };
    let mut page = page_at(1, 0);
    view.jump(&mut page, true);
    assert_eq!((page.selected, page.top), (2, 0));
}

// With nothing typed, Find lists the components exactly as before: no
// dividers, no days, no sessions.
#[test]
fn empty_find_lists_only_components() {
    let rows = rows(View::Find, &Runtime::default());
    let names: Vec<_> = fixture::COMPONENTS
        .iter()
        .map(|c| format!("{} {}", c.kind, c.name))
        .collect();
    let texts: Vec<_> = rows.iter().map(|r| r.text.clone()).collect();
    assert_eq!(texts, names);
}

// Sessions are grouped by day, then the largest first, so the cause of a
// day comes before the rest of it.
#[test]
fn find_orders_sessions_by_day_then_size() {
    let all = Data::fixture();
    let mut page = Page::new(View::Find);
    page.filter = "a".into();
    let rows = content(&page, &all, false, 80, &Runtime::default()).2;
    let ids: Vec<usize> = rows
        .iter()
        .filter_map(|r| match r.open {
            Some((View::Session(id), _)) => Some(id),
            _ => None,
        })
        .collect();
    assert!(ids.len() > 2);
    let keys: Vec<_> = ids
        .iter()
        .map(|&id| (all[id].day, std::cmp::Reverse(all[id].cents())))
        .collect();
    assert!(keys.windows(2).all(|w| w[0] <= w[1]), "{keys:?}");
}

/// Checks one marked cell: a figure holds a digit, and a secondary cell on
/// a Day row is that session's agent or its (shortened) workspace.
fn check_span(view: View, all: &[Session], text: &str, cell: &str, emphasis: Emphasis) {
    match emphasis {
        Emphasis::Figure => assert!(
            cell.chars().any(|c| c.is_ascii_digit()),
            "{view:?} figure {cell:?} in {text:?}"
        ),
        Emphasis::Secondary => {
            let View::Day(day) = view else {
                panic!("secondary on {view:?}")
            };
            let cell = cell.trim();
            let ok = all.iter().filter(|s| s.day == day).any(|s| {
                text.starts_with(s.name)
                    && (cell == s.agent || cell == shorten(s.workspace, cell.chars().count()))
            });
            assert!(ok, "{cell:?} in {text:?}");
        }
    }
}

// Spans must sit on the cells they name, so a builder whose columns
// change cannot leave bold or muted on the wrong text.
#[test]
fn every_span_marks_the_cell_it_names() {
    let rt = Runtime::default();
    for all in [Data::fixture(), Data::stress()] {
        let mut views = vec![View::Week];
        views.extend((12..=18).map(View::Day));
        views.extend((0..all.len()).flat_map(|id| [View::Session(id), View::Tokens(id)]));
        for width in [screen::list_width(80), screen::list_width(60)] {
            for count in [false, true] {
                for view in &views {
                    for row in content(&Page::new(*view), &all, count, width, &rt).2 {
                        let chars: Vec<char> = row.text.chars().collect();
                        for (at, emphasis) in &row.spans {
                            assert!(at.end <= chars.len(), "{view:?} {:?}", row.text);
                            let cell: String = chars[at.clone()].iter().collect();
                            check_span(*view, &all, &row.text, &cell, *emphasis);
                        }
                    }
                }
            }
        }
    }
}
