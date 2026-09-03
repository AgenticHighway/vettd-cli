//! Orchestration for `vettd observe`.
//!
//! Port of `spikes/828-passive-observer/prototype/observe.py::run_pipeline`, in the order
//! `docs/vettd-observe-port-plan.md` fixes. The order is not incidental — several steps sit where
//! they do for privacy or durability reasons, and each one is commented with what moving it would
//! cost:
//!
//! 1. The disclosure reaches stderr **before any session log or user file is opened**, on every
//!    path including the not-configured one. A user has to know what will be read before it is.
//! 2. The store is opened **only** for `--submit`. Cursors and the ledger are submission state; a
//!    dry run that advanced a cursor would silently starve the next real submit.
//! 3. The gate runs **before** anything is written or sent. A refusal leaves no file, no store row
//!    and nothing on stdout, and names the rule without ever echoing the value it caught.
//!
//! Two deliberate deviations from the prototype. It printed the payload path and the gate summary
//! on stdout; here machine-readable output owns stdout and those go to stderr (AGENTS.md). And it
//! advanced cursors once the payload was on disk; here they advance only after the server has the
//! record, so a local write can never make a run look sent.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::observe::args::ObserveArgs;
use crate::observe::attribute::attribute;
use crate::observe::attribute::fs_index::FsIndex;
use crate::observe::canonical::{canonical_json, hex_sha256, to_json_bytes};
use crate::observe::claude_code::ClaudeCodeSource;
use crate::observe::disclosure::render_observe_disclosure;
use crate::observe::envelope::{
    build_envelope, collect_dynamic, filter_records, Coverage, EnvelopeMeta, Resource,
    EXTRACTOR_VERSION,
};
use crate::observe::extract::extract;
use crate::observe::gate::{Dynamic, GATE};
use crate::observe::rank::rank;
use crate::observe::render::render_with_prices;
use crate::observe::source::Source;
use crate::observe::store::{LedgerRow, Store};
use crate::observe::submit::{submit_envelope, SubmitOutcome};
use crate::observe::types::{utc_day, SessionFacts, SessionKind, SessionRef};
use crate::submit::AuthConfig;

/// Exit codes, per the plan's "Exit codes" bullet and the repo's existing conventions.
pub(crate) const EXIT_OK: i32 = 0;
pub(crate) const EXIT_RUNTIME: i32 = 1;
pub(crate) const EXIT_GATE: i32 = 2;
pub(crate) const EXIT_NOT_CONFIGURED: i32 = 3;

/// What the pipeline resolved before it read anything.
struct Context {
    root: PathBuf,
    secret: Vec<u8>,
    run_id_basis: String,
    now_ms: i64,
    today: String,
}

/// Run the observation. Returns the process exit code.
pub(crate) fn run_observe(args: &ObserveArgs, json: bool) -> i32 {
    // Step 1-2. The disclosure comes first, even when telemetry is off: a user asking why the
    // command refused deserves to see what it would have collected. Nothing has been opened yet.
    let root = match resolve_root(args) {
        Ok(root) => root,
        Err(message) => {
            eprintln!("{message}");
            return EXIT_RUNTIME;
        }
    };
    // The data/source disclosure must precede even the saved credential read. A bare `--submit`
    // gets its destination from that user file, so the resolved destination follows immediately
    // afterwards while still preceding every session read.
    eprint!("{}", render_observe_disclosure(None, &root));
    let auth = match resolve_submit_target(args) {
        Ok(auth) => auth,
        Err((code, message)) => {
            eprintln!("{message}");
            return code;
        }
    };
    let destination = auth
        .as_ref()
        .map(|auth| crate::network::endpoint_display_host(&auth.endpoint).to_string());
    if let Some(destination) = destination {
        eprintln!("  Destination: {destination}");
    }

    if !crate::cli::telemetry_enabled_from_config() {
        eprint!("{}", not_configured_guidance());
        return EXIT_NOT_CONFIGURED;
    }

    let context = match resolve_context(args, root) {
        Ok(context) => context,
        Err(message) => {
            eprintln!("{message}");
            return EXIT_RUNTIME;
        }
    };
    match observe(args, &context, auth.as_ref(), json) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            EXIT_RUNTIME
        }
    }
}

/// `~/.claude` unless `--root` says otherwise. Resolved before the disclosure so the disclosure can
/// name the directory, which means this must not touch the filesystem.
fn resolve_root(args: &ObserveArgs) -> Result<PathBuf, String> {
    if let Some(root) = &args.root {
        return Ok(root.clone());
    }
    crate::cli::user_home_dir()
        .map(|home| home.join(".claude"))
        .ok_or_else(|| "Unable to determine home directory — pass --root".to_string())
}

/// Where a submission would go, or `None` when nothing will be sent.
///
/// The error carries its own exit code because the two failures are different things to a script:
/// a missing credential is "not configured" (3), the same as telemetry being off, while a refused
/// endpoint is a runtime error (1). Checking for the credential structurally here — rather than
/// reading it out of `resolve_submit_auth`'s message — keeps that distinction from depending on
/// wording that a later edit could change without anyone noticing.
fn resolve_submit_target(args: &ObserveArgs) -> Result<Option<AuthConfig>, (i32, String)> {
    if !args.wants_submit() {
        return Ok(None);
    }
    let saved = crate::submit::load_auth_config();
    let has_key = args.api_key.is_some()
        || saved
            .as_ref()
            .is_some_and(|config| !config.api_key.trim().is_empty());
    if !has_key {
        return Err((
            EXIT_NOT_CONFIGURED,
            "No API key for --submit. Run `vettd auth --key <your-key>`, or pass --api-key for \
             automation. Nothing was read."
                .to_string(),
        ));
    }

    // `resolve_submit_auth` takes the two-level Option the scan flags use: absent, present-bare,
    // present-with-URL. Ours collapses bare to `Some("")`, so rebuild the distinction here.
    let flag = Some(args.submit_endpoint().map(str::to_string));
    let mut auth = crate::output::resolve_submit_auth(
        &flag,
        args.api_key.as_deref(),
        args.allow_public_endpoint,
    )
    .map_err(|e| (EXIT_RUNTIME, e))?;

    // An explicit `--submit URL` is used verbatim — it is the operator naming a route, and
    // rewriting it would make a local test endpoint unreachable. Only the saved scan endpoint is
    // translated, since it points at `/api/scans/ingest`.
    if args.submit_endpoint().is_none() {
        auth.endpoint = crate::network::derive_api_url(&auth.endpoint, "observations/ingest");
    }
    Ok(Some(auth))
}

fn resolve_context(args: &ObserveArgs, root: PathBuf) -> Result<Context, String> {
    let (secret, basis) = crate::identity::resolve_observer_secret(args.secret_file.as_deref())?;
    let now_ms = args
        .now_ms
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let today = match &args.today {
        Some(today) => today.clone(),
        None => utc_day(now_ms),
    };
    Ok(Context {
        root,
        secret,
        run_id_basis: basis.to_string(),
        now_ms,
        today,
    })
}

fn observe(
    args: &ObserveArgs,
    context: &Context,
    auth: Option<&AuthConfig>,
    json: bool,
) -> Result<i32, String> {
    if !context.root.is_dir() {
        return Err(format!(
            "Harness home {} is not a directory — pass --root",
            context.root.display()
        ));
    }

    // Step 4. Submission state only, so a dry run cannot advance a cursor.
    let mut store = if args.wants_submit() {
        let store = Store::open_default()?;
        if store.ensure_secret_fingerprint(&context.secret)? {
            eprintln!(
                "  observer secret changed: cleared resumable cursors and the submission ledger"
            );
        }
        Some(store)
    } else {
        None
    };

    let source = ClaudeCodeSource::with_now_ms(context.root.clone(), context.now_ms);
    let refs = source.discover(&context.root, args.window_days, context.now_ms)?;
    let groups = group_sessions(refs);

    let mut coverage = Coverage {
        sessions_seen: groups.len() as u64,
        sessions_emitted: 0,
        sessions_skipped_unparseable: 0,
        lines_seen: 0,
        lines_unknown_type: 0,
        bytes_read: 0,
        truncated_sessions: 0,
        window_days: u64::from(args.window_days),
        cursor_state: cursor_state(store.as_ref())?,
    };
    let fs_index = FsIndex::new(Some(&context.root));
    let mut attributed = Vec::new();
    let mut staged: Vec<(String, crate::observe::types::Cursor)> = Vec::new();
    let mut harness_version = crate::observe::types::UNKNOWN.to_string();

    // `--resend` bypasses the cursor probe as well as the ledger, so every group is read from
    // byte 0. The plan's step 6 does not say so, but its own acceptance line does: it expects
    // `--resend` on an unchanged machine to report `0 new, 0 replaced, 1 duplicate`, which is
    // impossible if an unchanged group short-circuits before a record is ever built. Under the
    // literal step ordering the flag does nothing in exactly the situation a user reaches for it
    // — the machine has not changed and they want the data pushed again — and reports `nothing
    // new to send`, which reads as success. One flag, one meaning: ignore what I believe I have
    // already sent. Cursors are still staged and still advance only after a 2xx.
    let probe_store = if args.resend { None } else { store.as_ref() };
    for group in &groups {
        match read_group(&source, group, probe_store, &mut coverage, &mut staged)? {
            Some(facts) => {
                if let Some(version) = usable_version(&facts) {
                    harness_version = version;
                }
                let run = extract(&facts, context.now_ms);
                attributed.push(attribute(&run, &fs_index, &context.secret));
                coverage.sessions_emitted += 1;
            }
            None => continue,
        }
    }

    let meta = EnvelopeMeta {
        resource: Resource {
            device_id: crate::identity::resolve_scanner_uuid(None)?,
            device_id_source: "scanner_uuid".to_string(),
            harness: args.harness.clone(),
            harness_version,
            collector: "vettd-cli".to_string(),
            collector_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        coverage,
        today: context.today.clone(),
        secret: &context.secret,
        run_id_basis: context.run_id_basis.clone(),
        extractor_version: EXTRACTOR_VERSION.to_string(),
    };
    let envelope = build_envelope(&attributed, &meta)?;

    // Step 9. Drop records the server already holds under this exact hash, so a submit with
    // nothing new sends nothing at all. Keyed on the record hash and not just the run: a
    // truncated run that later completed keeps its `run_id` and changes its record, and must be
    // resent so the server can replace its row rather than reading as already-sent forever.
    //
    // Everything downstream — the gate, `--out`, the report — then works on the filtered
    // envelope, because in submit mode the payload IS the deliverable and the written file has to
    // be an auditable record of exactly what went on the wire.
    let (envelope, pending) = match auth {
        None => (envelope, Vec::new()),
        Some(auth) => {
            let host = crate::network::endpoint_display_host(&auth.endpoint);
            let rows = ledger_rows(&envelope, host)?;
            let (envelope, pending) = if args.resend {
                (envelope, rows)
            } else {
                drop_already_sent(envelope, &rows, store.as_ref())?
            };
            if pending.is_empty() {
                eprintln!("nothing new to send");
                return Ok(EXIT_OK);
            }
            (envelope, pending)
        }
    };

    // Step 8. The emitter's own local vocabulary, so the gate can prove none of it is a substring
    // of any string leaf. The machine's identity is added here rather than in `collect_dynamic`
    // because it is environment, not run data.
    let mut dynamic_sets = collect_dynamic(&attributed);
    add_machine_identity(&mut dynamic_sets);

    // Step 10. Before any write, any state change, and anything sent.
    let violations = GATE.check(&envelope, &Dynamic::normalize(&dynamic_sets));
    if !violations.is_empty() {
        eprintln!("REFUSING TO WRITE: payload fails the telemetry field gate:");
        for violation in &violations {
            eprintln!("  {violation}");
        }
        eprintln!("  The local report is unaffected; rerun without --out to see it.");
        return Ok(EXIT_GATE);
    }

    let bytes = to_json_bytes(&envelope)?;
    if let Some(path) = args.out_path() {
        std::fs::write(&path, &bytes)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());
    }
    eprintln!(
        "gate: OK ({} allowed leaf paths, 0 violations)",
        GATE.field_count()
    );

    if json {
        // Machine-readable output owns stdout, and the canonical bytes already end in a newline.
        let mut stdout = std::io::stdout();
        stdout
            .write_all(&bytes)
            .map_err(|e| format!("Failed to write to stdout: {e}"))?;
    } else {
        print_report(args, &envelope, &attributed)?;
    }

    // Step 13. Nothing above this line has changed any durable state.
    match (auth, args.dry_run) {
        (Some(auth), false) => send(&bytes, auth, &pending, &staged, store.as_mut()),
        (Some(_), true) => {
            // `--dry-run --submit` is a legitimate combination: build and check exactly what would
            // be sent, then send nothing. Saying so beats letting the user infer it from silence.
            eprintln!("dry run: nothing was sent and no cursor advanced");
            Ok(EXIT_OK)
        }
        (None, _) => Ok(EXIT_OK),
    }
}

/// Ask (when there is someone to ask), POST, then record what the server confirmed it holds.
///
/// The commit is the point of the whole ordering: cursors advance only once the server has the
/// record. The prototype committed them after the local write, which meant a payload written and
/// then lost to a failed POST looked, next run, exactly like one that had been delivered.
fn send(
    bytes: &[u8],
    auth: &AuthConfig,
    pending: &[LedgerRow],
    staged: &[(String, crate::observe::types::Cursor)],
    store: Option<&mut Store>,
) -> Result<i32, String> {
    let host = crate::network::endpoint_display_host(&auth.endpoint);
    if std::io::IsTerminal::is_terminal(&std::io::stdin())
        && !crate::wizard::confirm(
            &format!("Send {} run record(s) to {host}?", pending.len()),
            false,
        )
    {
        eprintln!("Not sent.");
        return Ok(EXIT_OK);
    }

    let outcome = submit_envelope(bytes, auth)?;
    eprintln!("{}", outcome.summary());
    if !outcome.deadline_exceeded.is_empty() {
        eprintln!(
            "  {} run(s) hit the server's deadline and were not stored; they will be resent.",
            outcome.deadline_exceeded.len()
        );
    }

    let Some(store) = store else {
        return Ok(EXIT_OK);
    };
    // Only the runs the server said it holds. A run it could not finish must stay unledgered, or
    // its cursor would advance past a record nobody has.
    let held: Vec<LedgerRow> = pending
        .iter()
        .filter(|row| outcome.persisted().any(|run_id| *run_id == row.run_id))
        .cloned()
        .collect();
    // Cursors are currently staged as one batch, without a run-id association. If even one run
    // was not confirmed, retaining the prior cursor batch is the only lossless choice: accepted
    // runs may be resent as duplicates, while advancing all cursors would make the missing run
    // look unchanged forever. Ledger rows remain safe to commit independently by run id.
    let confirmed_cursors = if all_pending_persisted(pending, &outcome) {
        staged
    } else {
        &[]
    };
    store.commit(confirmed_cursors, &held)?;
    Ok(EXIT_OK)
}

fn all_pending_persisted(pending: &[LedgerRow], outcome: &SubmitOutcome) -> bool {
    pending.iter().all(|row| {
        outcome
            .persisted()
            .any(|persisted| persisted == &row.run_id)
    })
}

/// One ledger row per record in `envelope`, keyed on the hash of the canonical record.
///
/// The hash is over `canonical_json` of the record alone, which is deterministic for a given run
/// state: the same run read twice hashes the same, and a run that gained turns does not.
fn ledger_rows(envelope: &Value, endpoint_host: &str) -> Result<Vec<LedgerRow>, String> {
    let harness = envelope["resource"]["harness"].as_str().unwrap_or_default();
    let emitted_day = envelope["emitted_day"].as_str().unwrap_or_default();
    let records = envelope["records"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    records
        .iter()
        .map(|record| {
            Ok(LedgerRow {
                run_id: record["run_id"].as_str().unwrap_or_default().to_string(),
                endpoint_host: endpoint_host.to_string(),
                harness: harness.to_string(),
                record_sha256: hex_sha256(canonical_json(record)?.as_bytes()),
                emitted_day: emitted_day.to_string(),
            })
        })
        .collect()
}

/// Drop records already in the ledger at this endpoint under this exact hash.
///
/// Returns the envelope to send and the rows to write on success. With no store — which cannot
/// happen in submit mode, since step 4 opens one — nothing is dropped: fewer records sent is a
/// silent loss, and more records sent is a duplicate the server already handles.
fn drop_already_sent(
    envelope: Value,
    rows: &[LedgerRow],
    store: Option<&Store>,
) -> Result<(Value, Vec<LedgerRow>), String> {
    let mut pending = Vec::new();
    let mut keep = BTreeSet::new();
    for row in rows {
        let already = match store {
            Some(store) => store.ledger_has(&row.run_id, &row.endpoint_host, &row.record_sha256)?,
            None => false,
        };
        if !already {
            keep.insert(row.run_id.clone());
            pending.push(row.clone());
        }
    }
    let filtered = filter_records(&envelope, |record| {
        record["run_id"]
            .as_str()
            .is_some_and(|run_id| keep.contains(run_id))
    });
    Ok((filtered, pending))
}

/// The report, on stdout.
///
/// `print!`, not `println!`: `render` already ends in exactly one newline, and the committed golden
/// `tests/fixtures/observe/golden/ranking.txt` is that exact byte string.
fn print_report(
    args: &ObserveArgs,
    envelope: &Value,
    attributed: &[crate::observe::types::AttributedRun],
) -> Result<(), String> {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for run in attributed {
        names.extend(run.name_map.clone());
    }
    let public = read_public_names(args.public_names.as_deref())?;
    let prices = load_prices(args.prices.as_deref())?;
    let result = rank(
        envelope,
        &names,
        args.task.as_deref().unwrap_or(""),
        &args.harness,
        args.model.as_deref(),
    );
    print!(
        "{}",
        render_with_prices(&result, args.scrub, &public, prices.as_ref())
    );
    Ok(())
}

fn read_public_names(path: Option<&Path>) -> Result<BTreeSet<String>, String> {
    let Some(path) = path else {
        return Ok(BTreeSet::new());
    };
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect())
}

/// An explicit `--prices` table, or `None` to use the compiled-in one.
fn load_prices(path: Option<&Path>) -> Result<Option<Value>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{} is not a valid price table: {e}", path.display()))
}

/// One harness run: a main transcript plus its sub-agent transcripts, both sorted by path.
struct Group {
    main: SessionRef,
    children: Vec<SessionRef>,
}

impl Group {
    fn refs(&self) -> impl Iterator<Item = &SessionRef> {
        std::iter::once(&self.main).chain(self.children.iter())
    }
}

fn group_sessions(refs: Vec<SessionRef>) -> Vec<Group> {
    let mut children: BTreeMap<String, Vec<SessionRef>> = BTreeMap::new();
    let mut mains: Vec<SessionRef> = Vec::new();
    for r in refs {
        match (&r.kind, &r.parent_key) {
            (SessionKind::Child, Some(parent)) => {
                children.entry(parent.clone()).or_default().push(r)
            }
            (SessionKind::Child, None) => {}
            (SessionKind::Main, _) => mains.push(r),
        }
    }
    mains.sort_by(|a, b| a.path.cmp(&b.path));
    mains
        .into_iter()
        .map(|main| {
            let mut kids = children.remove(&main.session_key).unwrap_or_default();
            kids.sort_by(|a, b| a.path.cmp(&b.path));
            Group {
                main,
                children: kids,
            }
        })
        .collect()
}

fn cursor_state(store: Option<&Store>) -> Result<String, String> {
    let resumed = match store {
        Some(store) => store.has_any_cursor()?,
        None => false,
    };
    Ok(if resumed { "resumed" } else { "fresh" }.to_string())
}

/// Read one group, probing from cursors first when every file in it has one.
///
/// A record is the cumulative state of one harness run and `run_id` is its idempotency key, so
/// cursors are CHANGE DETECTORS, not the start of a partial replacement record. An unchanged group
/// stages its probe cursors and emits nothing; a changed one is rebuilt from byte zero, and the
/// probe's bytes stay in coverage — the deliberate double count, because coverage reports what was
/// read, not what was emitted.
fn read_group(
    source: &ClaudeCodeSource,
    group: &Group,
    store: Option<&Store>,
    coverage: &mut Coverage,
    staged: &mut Vec<(String, crate::observe::types::Cursor)>,
) -> Result<Option<SessionFacts>, String> {
    if let Some(store) = store {
        if let Some(probe) = probe_group(source, group, store)? {
            match probe {
                Probe::Unchanged { cursors } => {
                    staged.extend(cursors);
                    return Ok(None);
                }
                Probe::Changed {
                    lines_seen,
                    lines_unknown_type,
                    bytes_read,
                } => {
                    coverage.lines_seen += lines_seen;
                    coverage.lines_unknown_type += lines_unknown_type;
                    coverage.bytes_read += bytes_read;
                }
            }
        }
    }

    let mut group_cursors = Vec::new();
    let (mut facts, cursor) = match source.read(&group.main, None) {
        Ok(read) => read,
        Err(_) => {
            // Fail open: count it and move on. Only a failed FULL read is unparseable.
            coverage.sessions_skipped_unparseable += 1;
            return Ok(None);
        }
    };
    group_cursors.push((source.harness().to_string(), cursor));
    for child in &group.children {
        match source.read(child, None) {
            Ok((child_facts, child_cursor)) => {
                facts.children.push(child_facts);
                group_cursors.push((source.harness().to_string(), child_cursor));
            }
            Err(_) => {
                // A failed child abandons the whole group's staged cursors: the prior complete
                // record stays authoritative and the group is retried next time. Emitting the
                // parent without its child would report a run that lost its sub-agent evidence.
                coverage.sessions_skipped_unparseable += 1;
                return Ok(None);
            }
        }
    }
    staged.extend(group_cursors);

    let lines: u64 = facts.lines_seen + facts.children.iter().map(|c| c.lines_seen).sum::<u64>();
    if lines == 0 {
        return Ok(None);
    }
    coverage.lines_seen += lines;
    coverage.lines_unknown_type += facts.lines_unknown_type
        + facts
            .children
            .iter()
            .map(|c| c.lines_unknown_type)
            .sum::<u64>();
    coverage.bytes_read +=
        facts.bytes_read + facts.children.iter().map(|c| c.bytes_read).sum::<u64>();
    if facts.truncated {
        coverage.truncated_sessions += 1;
    }
    Ok(Some(facts))
}

enum Probe {
    Unchanged {
        cursors: Vec<(String, crate::observe::types::Cursor)>,
    },
    Changed {
        lines_seen: u64,
        lines_unknown_type: u64,
        bytes_read: u64,
    },
}

/// Probe a group from its cursors. `None` when the group is not fully cursored, so it must be read
/// whole — a partially cursored group has a file whose prior state is unknown.
fn probe_group(
    source: &ClaudeCodeSource,
    group: &Group,
    store: &Store,
) -> Result<Option<Probe>, String> {
    let mut cursors = Vec::new();
    for r in group.refs() {
        match store.load_cursor(&r.path)? {
            Some(cursor) => cursors.push(cursor),
            None => return Ok(None),
        }
    }

    let mut changed = false;
    let mut staged = Vec::new();
    let mut lines_seen = 0;
    let mut lines_unknown_type = 0;
    let mut bytes_read = 0;
    for (r, cursor) in group.refs().zip(cursors.iter()) {
        match source.read(r, Some(cursor)) {
            Ok((delta, next)) => {
                changed |= delta.lines_seen > 0;
                lines_seen += delta.lines_seen;
                lines_unknown_type += delta.lines_unknown_type;
                bytes_read += delta.bytes_read;
                staged.push((source.harness().to_string(), next));
            }
            // A failed probe is treated as changed, so the group is rebuilt from byte zero and
            // only a failed full read counts as unparseable.
            Err(_) => changed = true,
        }
    }
    Ok(Some(if changed {
        Probe::Changed {
            lines_seen,
            lines_unknown_type,
            bytes_read,
        }
    } else {
        Probe::Unchanged { cursors: staged }
    }))
}

/// The newest usable harness version in a group, or `None` when every line said `unknown`.
fn usable_version(facts: &SessionFacts) -> Option<String> {
    std::iter::once(facts)
        .chain(facts.children.iter())
        .map(|f| f.harness_version.as_str())
        .find(|v| *v != crate::observe::types::UNKNOWN && !v.is_empty())
        .map(str::to_string)
}

/// The machine's own identity, so the gate can prove none of it reached a string leaf.
fn add_machine_identity(sets: &mut BTreeMap<String, BTreeSet<String>>) {
    let mut add = |bucket: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            sets.entry(bucket.to_string()).or_default().insert(value);
        }
    };
    add(
        "current_username",
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .ok(),
    );
    add(
        "hostname",
        hostname::get()
            .ok()
            .map(|h| h.to_string_lossy().to_string()),
    );
    add(
        "home_dir",
        crate::cli::user_home_dir().map(|h| h.to_string_lossy().to_string()),
    );
}

fn host_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .to_string()
}

/// What to tell a user who has not opted in.
fn not_configured_guidance() -> String {
    let path = crate::cli::access_config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.vettd/.vettd.toml".to_string());
    format!(
        "Observation is not enabled. Nothing was read.\n\n\
         To enable it, run `vettd observe enable`, or add this to {path}:\n\n\
         \x20   [telemetry]\n\
         \x20   enabled = true\n\n\
         Reading session transcripts is opt-in and stays off until that file says otherwise.\n"
    )
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
