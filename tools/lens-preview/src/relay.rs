//! `--relay`: an opt-in Claude Code statusline command that records the
//! 5-hour and weekly usage readings Claude passes on stdin, so
//! `--source local` can show Claude windows. It keeps four fields per
//! reading and nothing else, and it can't break the status line: it always
//! exits 0, prints at most one line, and reports errors on stderr only.
//!
//! This is the only mode that writes, and it writes only its own file.

use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const WINDOWS: [(&str, &str); 2] = [("five_hour", "5h"), ("seven_day", "7d")];
const DAY: i64 = 86_400;
/// Records older than this are pruned: 4 weekly windows plus a margin.
const KEEP_DAYS: i64 = 35;

/// One reading as stored: exactly these four fields.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Record {
    pub(crate) t: i64,
    pub(crate) w: String,
    pub(crate) pct: f64,
    pub(crate) resets_at: i64,
}

impl Record {
    fn line(&self) -> String {
        format!(
            "{}\n",
            serde_json::json!({"t": self.t, "w": self.w, "pct": self.pct, "resets_at": self.resets_at})
        )
    }

    fn parse(line: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(line).ok()?;
        Some(Self {
            t: v.get("t")?.as_i64()?,
            w: v.get("w")?.as_str()?.to_string(),
            pct: v.get("pct")?.as_f64()?,
            resets_at: v.get("resets_at")?.as_i64()?,
        })
    }
}

/// `~/.vettd/lens/relay.jsonl`, beside vettd's other user files.
pub(crate) fn default_file() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| {
            PathBuf::from(home)
                .join(".vettd")
                .join("lens")
                .join("relay.jsonl")
        })
}

/// Every record that parses. A cut-off line from a cancelled run is
/// skipped. None when there is no file.
pub(crate) fn records(file: &Path) -> Option<Vec<Record>> {
    let text = std::fs::read_to_string(file).ok()?;
    Some(text.lines().filter_map(Record::parse).collect())
}

/// The readings present in one statusline input. `spend_limit` and every
/// other field are ignored.
fn readings(input: &Value, now: i64) -> Vec<Record> {
    WINDOWS
        .iter()
        .filter_map(|(w, _)| {
            let r = input.get("rate_limits")?.get(*w)?;
            Some(Record {
                t: now,
                w: w.to_string(),
                pct: r.get("used_percentage")?.as_f64()?,
                resets_at: r.get("resets_at")?.as_i64()?,
            })
        })
        .collect()
}

/// `5h 23% · 7d 41%` for the windows present, or empty.
fn summary(readings: &[Record]) -> String {
    WINDOWS
        .iter()
        .filter_map(|(w, short)| {
            let r = readings.iter().find(|r| r.w == *w)?;
            Some(format!("{short} {}%", r.pct.round()))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Records the readings in `stdin` and returns what to print. Errors are
/// returned for stderr; they never change what is printed.
pub(crate) fn relay(
    stdin: &str,
    file: &Path,
    now: i64,
    then: Option<&str>,
) -> (String, Vec<String>) {
    let mut errors = Vec::new();
    let found = match serde_json::from_str::<Value>(stdin) {
        Ok(input) => readings(&input, now),
        Err(e) => {
            errors.push(format!("relay: stdin is not JSON ({e}); nothing recorded"));
            Vec::new()
        }
    };
    if !found.is_empty() {
        if let Err(e) = record(file, &found, now) {
            errors.push(format!("relay: can't write {}: {e}", file.display()));
        }
    }
    let out = match then {
        None => summary(&found),
        Some(command) => match chain(command, stdin) {
            Ok(out) => out,
            Err(e) => {
                errors.push(format!("relay: --then command failed: {e}"));
                String::new()
            }
        },
    };
    (out, errors)
}

/// Appends each changed reading as one whole line, then prunes if due.
fn record(file: &Path, found: &[Record], now: i64) -> std::io::Result<()> {
    if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let existing: Vec<Record> = text.lines().filter_map(Record::parse).collect();
    // After a cancelled run's cut-off line, start on a fresh line so this
    // record isn't glued to it and lost too.
    let fresh = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let changed: String = found
        .iter()
        .filter(|r| {
            let last = existing.iter().rev().find(|e| e.w == r.w);
            last.is_none_or(|e| e.pct != r.pct || e.resets_at != r.resets_at)
        })
        .map(Record::line)
        .collect();
    if !changed.is_empty() {
        // One write per run, so a cancelled run cuts at most this run's
        // lines; the reader skips a cut-off line.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)?
            .write_all(format!("{fresh}{changed}").as_bytes())?;
    }
    // Due when the oldest record is more than a day past the limit, so a
    // rewrite happens at most once a day without a marker file.
    let due = existing
        .first()
        .is_some_and(|r| r.t < now - (KEEP_DAYS + 1) * DAY);
    if due {
        prune(file, now)?;
    }
    Ok(())
}

/// Rewrites the file without records older than 35 days, through a
/// temporary file and a rename. A failure leaves the old file as it was.
pub(crate) fn prune(file: &Path, now: i64) -> std::io::Result<()> {
    prune_with(file, now, || {})
}

/// Other Claude sessions may append while the rewrite runs. Just before
/// the rename, whatever they appended since the snapshot is copied over,
/// so it isn't lost; only an append in the last instant before the rename
/// can still be. `between` lets tests append at that point.
fn prune_with(file: &Path, now: i64, between: impl FnOnce()) -> std::io::Result<()> {
    let text = std::fs::read_to_string(file)?;
    let kept = |text: &str| -> String {
        text.lines()
            .filter_map(Record::parse)
            .filter(|r| r.t >= now - KEEP_DAYS * DAY)
            .map(|r| r.line())
            .collect()
    };
    let tmp = file.with_extension(format!("jsonl.tmp-{}", std::process::id()));
    let result = std::fs::write(&tmp, kept(&text)).and_then(|_| {
        between();
        let latest = std::fs::read_to_string(file)?;
        if let Some(appended) = latest.get(text.len()..).filter(|a| !a.is_empty()) {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&tmp)?
                .write_all(kept(appended).as_bytes())?;
        }
        std::fs::rename(&tmp, file)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Runs the person's own statusline command with the same stdin and
/// returns its stdout.
fn chain(command: &str, stdin: &str) -> std::io::Result<String> {
    let mut child = shell(command).spawn_piped()?;
    if let Some(mut input) = child.stdin.take() {
        // A command that ignores stdin may close it early; that's fine.
        let _ = input.write_all(stdin.as_bytes());
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_string(&mut out)?;
    }
    child.wait()?;
    Ok(out.trim_end_matches(['\r', '\n']).to_string())
}

/// cmd.exe parses its command line itself, so the command goes in as
/// written: `/S /C "<command>"` strips exactly the outer quotes and keeps
/// quoted paths such as `"C:\Program Files\..."` intact. Rust's usual
/// argument escaping would turn those quotes into `\"`, which cmd.exe
/// doesn't read.
#[cfg(windows)]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("cmd");
    cmd.raw_arg("/S /C").raw_arg(format!("\"{command}\""));
    cmd
}

#[cfg(not(windows))]
fn shell(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", command]);
    cmd
}

trait SpawnPiped {
    fn spawn_piped(&mut self) -> std::io::Result<std::process::Child>;
}

impl SpawnPiped for Command {
    fn spawn_piped(&mut self) -> std::io::Result<std::process::Child> {
        self.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
    }
}

/// The `--relay` entry point: stdin in, one line out, exit 0.
pub(crate) fn run(file: Option<PathBuf>, then: Option<&str>) {
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let Some(file) = file.or_else(default_file) else {
        eprintln!("relay: no home directory; nothing recorded");
        return;
    };
    let (out, errors) = relay(&stdin, &file, now, then);
    for e in errors {
        eprintln!("{e}");
    }
    if !out.is_empty() {
        println!("{out}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_795_000_000;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lens-relay-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join("relay.jsonl")
    }

    fn cleanup(file: &Path) {
        let _ = std::fs::remove_dir_all(file.parent().unwrap().parent().unwrap());
    }

    const BOTH: &str = r#"{"session_id":"sess-secret","transcript_path":"/home/tester/t.jsonl",
        "cwd":"/home/tester/api","model":{"id":"claude-opus-5-5"},"cost":{"total_cost_usd":1.25},
        "rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1795003600},
        "seven_day":{"used_percentage":41.2,"resets_at":1795300000},
        "spend_limit":{"used_percentage":90,"resets_at":1}}}"#;

    #[test]
    fn records_both_windows_and_prints_one_line() {
        let file = tmp("both");
        let (out, errors) = relay(BOTH, &file, NOW, None);
        assert_eq!(out, "5h 24% · 7d 41%");
        assert!(errors.is_empty(), "{errors:?}");
        let records = records(&file).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].w, "five_hour");
        assert_eq!(records[0].pct, 23.5);
        cleanup(&file);
    }

    // The real feed sends whole-number percentages; they must be read
    // like fractional ones, not dropped as malformed.
    #[test]
    fn integer_percentages_are_recorded() {
        let file = tmp("integer");
        let input = r#"{"rate_limits":{"five_hour":{"used_percentage":7,"resets_at":1795003600},
            "seven_day":{"used_percentage":42,"resets_at":1795300000}}}"#;
        let (out, errors) = relay(input, &file, NOW, None);
        assert_eq!(out, "5h 7% · 7d 42%");
        assert!(errors.is_empty(), "{errors:?}");
        let pcts: Vec<f64> = records(&file).unwrap().iter().map(|r| r.pct).collect();
        assert_eq!(pcts, [7.0, 42.0]);
        cleanup(&file);
    }

    // The relay runs on every message of every session; only the four
    // reading fields may be kept, never what identifies the session.
    #[test]
    fn identifying_fields_and_spend_limit_are_not_stored() {
        let file = tmp("private");
        relay(BOTH, &file, NOW, None);
        let text = std::fs::read_to_string(&file).unwrap();
        for secret in [
            "sess-secret",
            "tester",
            "claude-opus",
            "1.25",
            "spend",
            "90",
        ] {
            assert!(!text.contains(secret), "{secret} stored");
        }
        for line in text.lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            assert_eq!(keys, ["pct", "resets_at", "t", "w"]);
        }
        cleanup(&file);
    }

    #[test]
    fn unchanged_reading_is_not_repeated() {
        let file = tmp("repeat");
        relay(BOTH, &file, NOW, None);
        relay(BOTH, &file, NOW + 60, None);
        assert_eq!(records(&file).unwrap().len(), 2);
        cleanup(&file);
    }

    #[test]
    fn absent_rate_limits_write_nothing_and_print_nothing() {
        let file = tmp("absent");
        let (out, errors) = relay(r#"{"session_id":"x"}"#, &file, NOW, None);
        assert_eq!(out, "");
        assert!(errors.is_empty());
        assert!(!file.exists(), "no file and no directory for nothing");
        assert!(!file.parent().unwrap().exists());
        cleanup(&file);
    }

    // A broken status line would hide Claude's own; bad input only goes
    // to stderr.
    #[test]
    fn bad_stdin_is_reported_and_ignored() {
        let file = tmp("bad");
        let (out, errors) = relay("not json", &file, NOW, None);
        assert_eq!(out, "");
        assert_eq!(errors.len(), 1);
        assert!(!file.exists());
        cleanup(&file);
    }

    #[test]
    fn unwritable_file_is_an_error_not_a_failure() {
        // A directory where the file should be.
        let file = tmp("unwritable");
        std::fs::create_dir_all(&file).unwrap();
        let (out, errors) = relay(BOTH, &file, NOW, None);
        assert_eq!(out, "5h 24% · 7d 41%");
        assert_eq!(errors.len(), 1, "{errors:?}");
        cleanup(&file);
    }

    #[test]
    fn then_prints_the_chained_command_after_recording() {
        let file = tmp("then");
        let (out, errors) = relay(BOTH, &file, NOW, Some("echo main ok"));
        assert_eq!(out, "main ok");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(records(&file).unwrap().len(), 2);
        cleanup(&file);
    }

    #[test]
    fn pruning_drops_records_older_than_35_days() {
        let file = tmp("prune");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let old = Record {
            t: NOW - 40 * DAY,
            w: "five_hour".into(),
            pct: 10.0,
            resets_at: NOW - 40 * DAY + 3600,
        };
        let recent = Record {
            t: NOW - 2 * DAY,
            w: "seven_day".into(),
            pct: 20.0,
            resets_at: NOW + DAY,
        };
        std::fs::write(&file, old.line() + &recent.line() + "{\"t\":1,").unwrap();
        // Recording runs the prune, since the oldest record is past due.
        relay(BOTH, &file, NOW, None);
        let kept = records(&file).unwrap();
        assert!(kept.iter().all(|r| r.t >= NOW - 35 * DAY), "{kept:?}");
        assert_eq!(kept.len(), 3, "the recent record and this run's two");
        cleanup(&file);
    }

    // Another session's append during a prune survives the rewrite.
    #[test]
    fn append_during_prune_is_kept() {
        let file = tmp("race");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let rec = |t: i64, pct: f64| Record {
            t,
            w: "five_hour".into(),
            pct,
            resets_at: NOW + 3600,
        };
        std::fs::write(
            &file,
            rec(NOW - 40 * DAY, 1.0).line() + &rec(NOW, 2.0).line(),
        )
        .unwrap();
        prune_with(&file, NOW, || {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap()
                .write_all(rec(NOW + 1, 3.0).line().as_bytes())
                .unwrap();
        })
        .unwrap();
        let pcts: Vec<f64> = records(&file).unwrap().iter().map(|r| r.pct).collect();
        assert_eq!(pcts, [2.0, 3.0]);
        cleanup(&file);
    }

    // A status command with a quoted path (as under "Program Files") must
    // reach the shell with its quotes intact.
    #[test]
    fn then_keeps_quoted_paths() {
        let file = tmp("quoted");
        let command = if cfg!(windows) {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            format!(r#""{root}\System32\cmd.exe" /C echo main ok"#)
        } else {
            r#""/bin/sh" -c "echo main ok""#.to_string()
        };
        let (out, errors) = relay(BOTH, &file, NOW, Some(&command));
        assert_eq!(out, "main ok", "{errors:?}");
        cleanup(&file);
    }

    #[test]
    fn cut_off_line_is_skipped_by_the_reader() {
        let file = tmp("cut");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "{\"t\":1,\"w\":\"five_hour\",\"pct\":3,\"resets_at\":9}\n{\"t\":2,\"w\":\"fi",
        )
        .unwrap();
        assert_eq!(records(&file).unwrap().len(), 1);
        cleanup(&file);
    }
}
