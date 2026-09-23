//! Tests for [`super`], ported from the `BomVersion` and `Settle` cases of
//! `spikes/828-passive-observer/prototype/tests/test_attribute.py` and extended to cover each
//! disqualifying condition of the settle rule on its own.
//!
//! Every name, server and timestamp below is invented. The `bom_version` digests are not
//! recomputed in Rust: they were produced by running the Python `attribute.bom_version` oracle on
//! the same inputs, so they pin the port to the prototype rather than to itself.

use super::*;

/// Invented harness listing timestamp (ms), the same value `test_attribute.py` uses.
const T: i64 = 1_756_000_000_000;

fn initial(ts_ms: i64) -> LoadedSetEvent {
    LoadedSetEvent {
        ts_ms,
        kind: LoadedSetKind::Initial,
        ..LoadedSetEvent::default()
    }
}

fn delta(ts_ms: i64) -> LoadedSetEvent {
    LoadedSetEvent {
        ts_ms,
        kind: LoadedSetKind::Delta,
        ..LoadedSetEvent::default()
    }
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn pending(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn member_strings(seg: &SegState) -> Vec<String> {
    seg.members()
        .into_iter()
        .map(|(asset_type, name)| format!("{asset_type}:{name}"))
        .collect()
}

// -- mcp_server_of -------------------------------------------------------------------------------

/// The server of a tool name decides both segment membership and whether a delta folds, so its
/// edge cases are load-bearing. Every expectation below is the value the Python `mcp_server_of`
/// printed for the same input. Cannot prove the harness only ever emits these shapes.
#[test]
fn mcp_server_of_matches_the_python_table() {
    assert_eq!(mcp_server_of("mcp__srv__tool"), Some("srv"));
    assert_eq!(mcp_server_of("mcp__srv__a__b"), Some("srv"));
    assert_eq!(mcp_server_of("mcp__srv__"), Some("srv"));
    assert_eq!(mcp_server_of("mcp____tool"), None);
    assert_eq!(mcp_server_of("mcp__srv"), None);
    assert_eq!(mcp_server_of("mcp__"), None);
    assert_eq!(mcp_server_of("Bash"), None);
    assert_eq!(mcp_server_of(""), None);
}

// -- bom_version ---------------------------------------------------------------------------------

/// `bom_version` is a set hash: the same ids in another order, or with duplicates, must give the
/// same value, because the cloud de-duplicates `bom[]` entries by it and two segments with the
/// same assets must share one entry. A different set must give a different value.
/// Cannot prove collision resistance beyond sha256's.
#[test]
fn bom_version_is_order_independent_and_set_sensitive() {
    let ids = [
        "0123456789abcdef".repeat(4),
        "f".repeat(64),
        format!("{}1", "0".repeat(63)),
    ];
    let forward: Vec<&str> = ids.iter().map(String::as_str).collect();
    let reversed: Vec<&str> = forward.iter().rev().copied().collect();
    let mut duplicated = forward.clone();
    duplicated.extend_from_slice(&forward);

    assert_eq!(bom_version(forward.clone()), bom_version(reversed));
    assert_eq!(bom_version(forward.clone()), bom_version(duplicated));
    assert_ne!(
        bom_version(forward.clone()),
        bom_version(forward[..2].to_vec())
    );
}

/// Byte-level parity with the Python oracle. These four digests were printed by
/// `attribute.bom_version` on the same inputs; if the join separator, the de-duplication or the
/// sort order ever drifts, one of them changes. The last vector is deliberately mixed-case with a
/// digit and an underscore: `9` < `Z` < `_` < `a` by code point, so a locale- or case-insensitive
/// sort would fail it. Cannot prove the *choice* of preimage is right — only that it is unchanged.
#[test]
fn bom_version_matches_the_python_oracle() {
    assert_eq!(
        bom_version(Vec::<&str>::new()),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "the empty set must hash the empty string: a segment may load nothing"
    );
    let b64 = "b".repeat(64);
    let a64 = "a".repeat(64);
    assert_eq!(
        bom_version(vec![b64.as_str(), a64.as_str(), b64.as_str()]),
        "b1da1a1755748f4703baeaa4ae58e2f42a25bd33aa67a0db9fd84d6b8767c492"
    );
    let wide = "0123456789abcdef".repeat(4);
    let f64s = "f".repeat(64);
    let low = format!("{}1", "0".repeat(63));
    assert_eq!(
        bom_version(vec![wide.as_str(), f64s.as_str(), low.as_str()]),
        "323aa95e60b0d9a2feeef0d9fec207e6e2ef3414d1cd080f56c6edcacdc5cfc3"
    );
    assert_eq!(
        bom_version(vec!["Zebra", "apple", "_under", "9nine", "Zebra"]),
        "1c5309e8477eb5de2d6c2365e8ea6c7b0aca252d9ea46fc3d4037ea7f121b3c0"
    );
}

// -- the settle rule -----------------------------------------------------------------------------

/// A run with one listing is one segment opening at the run's first timestamp, not at the
/// listing's: the segment covers the whole run, including the turns before the harness announced
/// what it had loaded. Cannot prove anything about a run with no events at all beyond this shape.
#[test]
fn a_single_initial_event_is_one_segment_starting_at_the_run_start() {
    let events = vec![LoadedSetEvent {
        skills: names(&["skill-ghost"]),
        ..initial(T)
    }];
    let segs = settle(&events, T - 1_000);

    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].index, 0);
    assert_eq!(segs[0].start_ts, T - 1_000);
    assert_eq!(member_strings(&segs[0]), vec!["skill:skill-ghost"]);
}

/// The rule this module exists for: a delta that only adds tools of a server an earlier event
/// reported pending is an async MCP connect completing, not a configuration change, so it must
/// not cut a segment. The server still becomes a member and its schema bytes still accumulate.
/// Cannot prove every real async-connect delta has this shape.
#[test]
fn pending_mcp_completion_folds_into_the_segment() {
    let events = vec![
        LoadedSetEvent {
            tool_names: names(&["Bash"]),
            pending_mcp: names(&["srvfx"]),
            ..initial(T)
        },
        LoadedSetEvent {
            tool_names: names(&["mcp__srvfx__list"]),
            tool_schema_bytes: BTreeMap::from([("srvfx".to_string(), 402)]),
            ..delta(T + 13_000)
        },
    ];
    let segs = settle(&events, T);

    assert_eq!(segs.len(), 1);
    assert_eq!(member_strings(&segs[0]), vec!["mcp_server:srvfx"]);
    assert_eq!(segs[0].schema_bytes.get("srvfx"), Some(&402));
}

/// A delta carrying nothing at all folds, because `all()` over no added tools is vacuously true.
/// This is not a curiosity: a heartbeat-shaped delta must not inflate `loaded_set_changes`.
/// Cannot prove the reader ever produces such an event.
#[test]
fn an_empty_delta_folds() {
    assert!(folds(&delta(T), &BTreeSet::new()));
    assert_eq!(settle(&[initial(T), delta(T + 1_000)], T).len(), 1);
}

/// Each of the five conditions disqualifies a fold **on its own**, with no added tools present to
/// confuse the result: a removal, a re-add, a skill, an agent or a rules file each mean the set of
/// loaded things really changed. Tested separately so a future edit cannot delete one clause and
/// stay green on the others. Cannot prove these are the only changes a harness can make.
#[test]
fn each_disqualifying_condition_blocks_the_fold_on_its_own() {
    let all_pending = pending(&["srvfx", "srvzz"]);
    let cases: [(&str, LoadedSetEvent); 5] = [
        (
            "removed",
            LoadedSetEvent {
                removed: names(&["mcp__srvfx__list"]),
                ..delta(T)
            },
        ),
        (
            "readded",
            LoadedSetEvent {
                readded: names(&["mcp__srvfx__list"]),
                ..delta(T)
            },
        ),
        (
            "skills",
            LoadedSetEvent {
                skills: names(&["skill-ghost"]),
                ..delta(T)
            },
        ),
        (
            "agent_types",
            LoadedSetEvent {
                agent_types: names(&["agent-omega"]),
                ..delta(T)
            },
        ),
        (
            "rules_files",
            LoadedSetEvent {
                rules_files: names(&["RULES.md"]),
                ..delta(T)
            },
        ),
    ];
    for (label, ev) in cases {
        assert!(
            !folds(&ev, &all_pending),
            "a delta with {label} must not fold"
        );
        let segs = settle(&[initial(T - 1), ev], T - 1);
        assert_eq!(segs.len(), 2, "a delta with {label} must cut a segment");
    }
}

/// A built-in agent type is not an asset, but it is still an *agent* in the delta, so it
/// disqualifies a fold exactly like any other agent name. The Python filters built-ins in
/// `absorb`, never in `folds`. Cannot prove the harness distinguishes the two cases itself.
#[test]
fn a_builtin_agent_in_a_delta_still_blocks_the_fold() {
    let ev = LoadedSetEvent {
        agent_types: names(&["Explore"]),
        ..delta(T)
    };
    assert!(!folds(&ev, &pending(&["srvfx"])));
    let segs = settle(&[initial(T - 1), ev], T - 1);
    assert_eq!(segs.len(), 2);
    assert!(
        member_strings(&segs[1]).is_empty(),
        "built-ins are never members"
    );
}

/// The quantifier is **every** added tool, not any: a delta that completes one announced server
/// and simultaneously brings in a server nobody announced is a configuration change and must
/// split. Getting this backwards would silently merge real changes into one segment.
/// Cannot prove which of the two servers caused the change.
#[test]
fn a_delta_only_partly_in_pending_does_not_fold() {
    let prior = pending(&["srvfx"]);
    let ev = LoadedSetEvent {
        tool_names: names(&["mcp__srvfx__list", "mcp__srvzz__ping"]),
        ..delta(T + 1_000)
    };
    assert!(!folds(&ev, &prior));

    let only_announced = LoadedSetEvent {
        tool_names: names(&["mcp__srvfx__list"]),
        ..delta(T + 1_000)
    };
    assert!(
        folds(&only_announced, &prior),
        "the announced half alone would fold"
    );
}

/// A tool that is not `mcp__<server>__*` has no server, so it can never be in the pending set and
/// can never fold: a new built-in tool appearing mid-run is a change to what is loaded.
/// Cannot prove the harness never renames a built-in.
#[test]
fn a_non_mcp_added_tool_never_folds() {
    let ev = LoadedSetEvent {
        tool_names: names(&["Bash"]),
        ..delta(T)
    };
    assert!(!folds(&ev, &pending(&["srvfx", "Bash"])));
}

/// `pending` is cumulative and strictly prior: an announcement three events back still licenses a
/// fold, but an event's own `pending_mcp` does not license itself. The Python seeds the set once
/// before the loop and updates it *after* judging each event; this test is what pins that
/// ordering. Cannot prove a harness never re-announces a server it already connected.
#[test]
fn pending_is_cumulative_across_events_and_never_licenses_its_own_event() {
    let late_completion = vec![
        LoadedSetEvent {
            pending_mcp: names(&["srvfx"]),
            ..initial(T)
        },
        delta(T + 1_000),
        delta(T + 2_000),
        LoadedSetEvent {
            tool_names: names(&["mcp__srvfx__list"]),
            ..delta(T + 3_000)
        },
    ];
    assert_eq!(settle(&late_completion, T).len(), 1);

    let self_announced = vec![
        initial(T),
        LoadedSetEvent {
            tool_names: names(&["mcp__srvfx__list"]),
            pending_mcp: names(&["srvfx"]),
            ..delta(T + 1_000)
        },
    ];
    assert_eq!(settle(&self_announced, T).len(), 2);
}

/// An `initial` event never splits, whatever it carries: a harness that re-announces its whole
/// listing has not changed anything. Ported from `test_unexplained_addition_or_readd_splits`.
/// Cannot prove the harness only re-announces when nothing changed.
#[test]
fn a_second_initial_event_never_splits() {
    let events = vec![
        LoadedSetEvent {
            skills: names(&["skill-ghost"]),
            ..initial(T)
        },
        LoadedSetEvent {
            agent_types: names(&["agent-omega"]),
            ..initial(T + 1_000)
        },
    ];
    let segs = settle(&events, T);

    assert_eq!(segs.len(), 1);
    assert_eq!(
        member_strings(&segs[0]),
        vec!["agent:agent-omega", "skill:skill-ghost"]
    );
}

/// Segments are numbered by position and open at the timestamp of the delta that cut them, and a
/// folding delta between two splitting ones does not shift the numbering. `index` is what the
/// envelope carries and what observations are keyed by, so an off-by-one here would misattribute
/// every invocation. Cannot prove the harness timestamps are monotonic.
#[test]
fn segments_are_numbered_by_position_and_open_at_the_splitting_delta() {
    let events = vec![
        LoadedSetEvent {
            tool_names: names(&["Bash"]),
            pending_mcp: names(&["srvfx"]),
            ..initial(T)
        },
        // folds: srvfx was announced pending
        LoadedSetEvent {
            tool_names: names(&["mcp__srvfx__list"]),
            ..delta(T + 1_000)
        },
        // splits: srvzz was never announced
        LoadedSetEvent {
            tool_names: names(&["mcp__srvzz__ping"]),
            ..delta(T + 2_000)
        },
        // splits: a removal
        LoadedSetEvent {
            removed: names(&["mcp__srvfx__list"]),
            ..delta(T + 3_000)
        },
    ];
    let segs = settle(&events, T);

    assert_eq!(
        segs.iter().map(|seg| seg.index).collect::<Vec<usize>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        segs.iter().map(|seg| seg.start_ts).collect::<Vec<i64>>(),
        vec![T, T + 2_000, T + 3_000]
    );
    // The same three member sets the Python `settle` produces for these events.
    assert_eq!(
        segs.iter()
            .map(member_strings)
            .collect::<Vec<Vec<String>>>(),
        vec![
            vec!["mcp_server:srvfx"],
            vec!["mcp_server:srvfx", "mcp_server:srvzz"],
            vec!["mcp_server:srvzz"],
        ]
    );
}

// -- membership ----------------------------------------------------------------------------------

/// A fork carries the loaded set forward and the removal then applies only to the new segment:
/// the server is a member of segment 0 and not of segment 1, and the two `bom_version`s differ.
/// Ported from `test_removal_splits_the_segment`. Cannot prove the removal timing beyond the
/// harness timestamp.
#[test]
fn a_removal_drops_the_server_from_the_new_segment_only() {
    let events = vec![
        LoadedSetEvent {
            tool_names: names(&["mcp__srvfx__list"]),
            ..initial(T)
        },
        LoadedSetEvent {
            removed: names(&["mcp__srvfx__list"]),
            ..delta(T + 5_000)
        },
    ];
    let segs = settle(&events, T);

    assert_eq!(member_strings(&segs[0]), vec!["mcp_server:srvfx"]);
    assert!(member_strings(&segs[1]).is_empty());
    // Diverging membership must diverge the bom_version, which is what makes a segment boundary
    // visible in the envelope. Real asset ids are hashes built in `attribute/mod.rs`; the member
    // strings stand in for them here, and the property under test is the same.
    let ids_before: Vec<String> = member_strings(&segs[0]);
    let ids_after: Vec<String> = member_strings(&segs[1]);
    assert_ne!(
        bom_version(ids_before.iter().map(String::as_str)),
        bom_version(ids_after.iter().map(String::as_str))
    );
}

/// A server is a member while it has at least one live tool: removing one of two tools leaves it
/// loaded, and removing the last one drops it. This is why membership is tracked at tool
/// granularity rather than by server name. Cannot prove the harness reports every tool removal.
#[test]
fn a_server_stays_loaded_until_its_last_tool_is_removed() {
    let events = vec![
        LoadedSetEvent {
            tool_names: names(&["mcp__srvfx__list", "mcp__srvfx__read"]),
            ..initial(T)
        },
        LoadedSetEvent {
            removed: names(&["mcp__srvfx__list"]),
            ..delta(T + 1_000)
        },
        LoadedSetEvent {
            removed: names(&["mcp__srvfx__read"]),
            ..delta(T + 2_000)
        },
    ];
    let segs = settle(&events, T);

    assert_eq!(member_strings(&segs[1]), vec!["mcp_server:srvfx"]);
    assert!(member_strings(&segs[2]).is_empty());
}

/// Built-in agent types are not assets, so they never become members even though the delta names
/// them; a non-built-in agent in the same event does. Cannot prove the built-in list is complete.
#[test]
fn builtin_agent_types_are_never_members() {
    let events = vec![LoadedSetEvent {
        agent_types: names(&["Explore", "general-purpose", "agent-omega"]),
        skills: names(&["skill-ghost"]),
        rules_files: names(&["RULES.md"]),
        ..initial(T)
    }];
    let segs = settle(&events, T);

    assert_eq!(
        member_strings(&segs[0]),
        vec![
            "agent:agent-omega",
            "rules_file:RULES.md",
            "skill:skill-ghost"
        ]
    );
}

/// The first listing that names an asset wins the timestamp, and byte counts sum across events.
/// The timestamp is what the mtime binding is proven against, so a later re-listing must not move
/// it: doing so would let a file edited after the load still look `mtime_proven`.
/// Cannot prove the harness timestamps correspond to when the content was actually read.
#[test]
fn listed_ts_keeps_the_first_listing_and_byte_counts_accumulate() {
    let events = vec![
        LoadedSetEvent {
            skills: names(&["skill-ghost"]),
            listing_bytes: BTreeMap::from([("skill-ghost".to_string(), 100)]),
            tool_names: names(&["mcp__srvfx__list"]),
            tool_schema_bytes: BTreeMap::from([("srvfx".to_string(), 400)]),
            pending_mcp: names(&["srvfx"]),
            ..initial(T)
        },
        LoadedSetEvent {
            listing_bytes: BTreeMap::from([("skill-ghost".to_string(), 21)]),
            tool_names: names(&["mcp__srvfx__read"]),
            tool_schema_bytes: BTreeMap::from([("srvfx".to_string(), 2)]),
            ..delta(T + 9_000)
        },
    ];
    let segs = settle(&events, T);

    assert_eq!(segs.len(), 1, "the second event folds");
    assert_eq!(
        segs[0]
            .listed_ts
            .get(&("skill".to_string(), "skill-ghost".to_string())),
        Some(&T)
    );
    assert_eq!(
        segs[0]
            .listed_ts
            .get(&("mcp_server".to_string(), "srvfx".to_string())),
        Some(&T),
        "the server keeps the timestamp of the event that first listed one of its tools"
    );
    assert_eq!(segs[0].listing_bytes.get("skill-ghost"), Some(&121));
    assert_eq!(segs[0].schema_bytes.get("srvfx"), Some(&402));
}

/// `members()` is sorted by `(asset_type, name)` in code-point order, because observations are
/// built in that order and the golden envelope is compared byte for byte downstream.
/// Cannot prove the downstream sort key stays the same.
#[test]
fn members_are_sorted_by_type_then_name() {
    let mut seg = SegState::new(0, T);
    for name in ["zeta", "Alpha", "_mid"] {
        seg.add(ASSET_SKILL, name, Some(T));
    }
    seg.add(ASSET_AGENT, "omega", Some(T));

    assert_eq!(
        member_strings(&seg),
        vec!["agent:omega", "skill:Alpha", "skill:_mid", "skill:zeta"]
    );
}

// -- boundaries ----------------------------------------------------------------------------------

/// An invocation is attributed to the last segment that had already opened, and anything before
/// the first boundary lands in segment 0 rather than being dropped. Cannot prove the harness and
/// the tool-call clocks agree.
#[test]
fn segment_for_picks_the_last_segment_that_had_opened() {
    let segs = settle(
        &[
            initial(T),
            LoadedSetEvent {
                removed: names(&["mcp__srvfx__x"]),
                ..delta(T + 5_000)
            },
            LoadedSetEvent {
                removed: names(&["mcp__srvzz__x"]),
                ..delta(T + 9_000)
            },
        ],
        T,
    );

    assert_eq!(segment_for(&segs, T - 10_000), 0, "before the run start");
    assert_eq!(segment_for(&segs, T), 0);
    assert_eq!(segment_for(&segs, T + 4_999), 0);
    assert_eq!(segment_for(&segs, T + 5_000), 1, "a boundary is inclusive");
    assert_eq!(segment_for(&segs, T + 9_000), 2);
    assert_eq!(segment_for(&segs, T + 1_000_000), 2);
}

/// A segment ends where the next opens, and the last ends at the run's last timestamp — floored at
/// its own start, so a listing that arrives after the last recorded activity still yields a
/// non-negative span. Cannot prove `last_ts_ms` is the true end of the run.
#[test]
fn end_ts_is_the_next_start_and_the_run_end_for_the_last_segment() {
    let segs = settle(
        &[
            initial(T),
            LoadedSetEvent {
                removed: names(&["mcp__srvfx__x"]),
                ..delta(T + 5_000)
            },
        ],
        T,
    );

    assert_eq!(end_ts_for(&segs, 0, T + 100_000), T + 5_000);
    assert_eq!(end_ts_for(&segs, 1, T + 100_000), T + 100_000);
    assert_eq!(
        end_ts_for(&segs, 1, T),
        T + 5_000,
        "a run whose last activity precedes the split cannot end before the segment starts"
    );
}

/// The basis is a closed enum and a harness listing always wins over the filesystem; with neither,
/// the segment reports `none` rather than pretending. The three strings are what the gate allows
/// for `loaded_set_basis`. Cannot prove the caller passes the booleans correctly.
#[test]
fn loaded_set_basis_prefers_the_harness_listing() {
    assert_eq!(loaded_set_basis(true, true), BASIS_HARNESS_LOG);
    assert_eq!(loaded_set_basis(true, false), BASIS_HARNESS_LOG);
    assert_eq!(loaded_set_basis(false, true), BASIS_FILESYSTEM);
    assert_eq!(loaded_set_basis(false, false), BASIS_NONE);
    assert_eq!(
        [BASIS_HARNESS_LOG, BASIS_FILESYSTEM, BASIS_NONE],
        ["harness_log", "filesystem", "none"]
    );
}

/// A fork copies membership, timestamps, byte counts and the corroboration map rather than
/// aliasing them: mutating the new segment must not reach back into the finished one, which is
/// what makes a per-segment `bom_version` meaningful. Cannot prove callers never mutate a
/// finished segment directly.
#[test]
fn fork_deep_copies_the_state_it_carries_forward() {
    let mut first = SegState::new(0, T);
    first.add(ASSET_SKILL, "skill-ghost", Some(T));
    first.mcp_corroborations.insert("srvfx".to_string(), 3);
    first.listing_bytes.insert("skill-ghost".to_string(), 100);

    let mut second = first.fork(1, T + 1_000);
    second.add(ASSET_SKILL, "skill-new", Some(T + 1_000));
    second.listing_bytes.insert("skill-ghost".to_string(), 7);

    assert_eq!(member_strings(&first), vec!["skill:skill-ghost"]);
    assert_eq!(
        member_strings(&second),
        vec!["skill:skill-ghost", "skill:skill-new"]
    );
    assert_eq!(first.listing_bytes.get("skill-ghost"), Some(&100));
    assert_eq!(second.mcp_corroborations.get("srvfx"), Some(&3));
    assert_eq!((second.index, second.start_ts), (1, T + 1_000));
}
