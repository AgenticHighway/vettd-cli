pub mod base;
pub mod browser_footprints;
pub mod containers;
pub mod custom_rules;
pub mod mcp_configs;
pub mod source_risks;

use base::Detector;
use std::fs::File;
use std::io::Read;
use std::path::Path;

fn read_utf8_head(path: &Path, max_bytes: usize) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut limited = file.take(max_bytes as u64);
    let mut bytes = Vec::with_capacity(max_bytes);
    limited.read_to_end(&mut bytes).ok()?;
    match String::from_utf8(bytes) {
        Ok(content) => Some(content),
        Err(err) => {
            let valid_up_to = err.utf8_error().valid_up_to();
            if valid_up_to == 0 {
                return None;
            }
            let bytes = err.into_bytes();
            std::str::from_utf8(&bytes[..valid_up_to])
                .ok()
                .map(str::to_owned)
        }
    }
}

// ---------------------------------------------------------------------------
// Detector registration contract
// ---------------------------------------------------------------------------
//
// This table is the single place that ties a detector's name to which scan
// modes run it. Adding a detector means adding one `DetectorSpec` entry here
// — not hand-syncing separate match statements across this module.
//
// Only modes the CLI can actually dispatch (see
// `cli.rs::resolve_scan_params`) belong in `modes`: "host", "scan", "root",
// "workdir", "file".

const ALL_MODES: &[&str] = &["host", "scan", "root", "workdir", "file"];

struct DetectorSpec {
    name: &'static str,
    modes: &'static [&'static str],
}

const DETECTOR_SPECS: &[DetectorSpec] = &[
    DetectorSpec {
        name: "custom_rules",
        modes: ALL_MODES,
    },
    DetectorSpec {
        name: "containers",
        modes: ALL_MODES,
    },
    DetectorSpec {
        name: "mcp_configs",
        modes: ALL_MODES,
    },
    DetectorSpec {
        name: "source_risks",
        modes: &["workdir", "file"],
    },
    DetectorSpec {
        name: "browser_footprints",
        modes: &["host", "scan", "root"],
    },
];

fn make_detector(name: &str, mode: &str) -> Box<dyn Detector> {
    match name {
        "custom_rules" => Box::new(custom_rules::CustomRulesDetector::load()),
        "containers" => Box::new(containers::ContainerDetector),
        "mcp_configs" => Box::new(mcp_configs::MCPConfigDetector),
        "source_risks" => Box::new(source_risks::SourceRiskDetector::new(mode)),
        "browser_footprints" => Box::new(browser_footprints::BrowserFootprintDetector),
        other => unreachable!("DETECTOR_SPECS and make_detector are out of sync: {other}"),
    }
}

pub fn get_all_detectors(mode: &str) -> Vec<Box<dyn Detector>> {
    DETECTOR_SPECS
        .iter()
        .filter(|spec| spec.modes.contains(&mode))
        .map(|spec| make_detector(spec.name, mode))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn workdir_mode_has_four_detectors() {
        let detectors = get_all_detectors("workdir");
        assert_eq!(detectors.len(), 4);
        assert!(detectors.iter().any(|d| d.name() == "source_risks"));
    }

    #[test]
    fn host_mode_includes_browser_detector() {
        let detectors = get_all_detectors("host");
        assert_eq!(detectors.len(), 4);
        assert!(detectors.iter().any(|d| d.name() == "browser_footprints"));
    }

    #[test]
    fn scan_mode_includes_browser_detector() {
        let detectors = get_all_detectors("scan");
        assert_eq!(detectors.len(), 4);
        assert!(detectors.iter().any(|d| d.name() == "browser_footprints"));
    }

    #[test]
    fn root_mode_includes_browser_detector() {
        let detectors = get_all_detectors("root");
        assert_eq!(detectors.len(), 4);
    }

    #[test]
    fn file_mode_excludes_browser_detector() {
        let detectors = get_all_detectors("file");
        assert!(detectors.iter().any(|d| d.name() == "source_risks"));
        assert!(!detectors.iter().any(|d| d.name() == "browser_footprints"));
    }

    #[test]
    fn all_detectors_have_names() {
        let detectors = get_all_detectors("host");
        for d in &detectors {
            assert!(!d.name().is_empty());
        }
    }

    #[test]
    fn unreachable_legacy_modes_get_no_detectors() {
        // "filesystem" and "home" are legacy run_scan modes the CLI never
        // dispatches (see cli.rs::resolve_scan_params) and are deliberately
        // absent from ALL_MODES/DETECTOR_SPECS — they used to silently
        // match `matches!(mode, "host" | "scan" | "filesystem" | "home" |
        // "root")` for browser_footprints, which is exactly the kind of
        // hand-synced, easy-to-miss mode gating this registration table
        // exists to prevent.
        for mode in ["filesystem", "home"] {
            let detectors = get_all_detectors(mode);
            assert!(
                detectors.is_empty(),
                "unreachable mode '{mode}' should register no detectors"
            );
        }
    }

    #[test]
    fn every_detector_spec_name_is_constructible() {
        // Guards against DETECTOR_SPECS and make_detector() drifting apart:
        // every registered name must be constructible in every mode it
        // claims to run in (make_detector panics on an unknown name).
        for spec in DETECTOR_SPECS {
            for mode in spec.modes {
                let _ = make_detector(spec.name, mode);
            }
        }
    }

    #[test]
    fn every_detector_spec_constructed_name_matches_its_registration() {
        // Guards against a copy-paste mistake where a spec's `name` field
        // and the detector `make_detector` actually builds for it disagree
        // — that would make `get_all_detectors` reason about one name while
        // the running detector reports another, silently breaking mode gating
        // for it.
        for spec in DETECTOR_SPECS {
            for mode in spec.modes {
                let detector = make_detector(spec.name, mode);
                assert_eq!(
                    detector.name(),
                    spec.name,
                    "make_detector('{}', '{}') built a detector reporting a different name",
                    spec.name,
                    mode
                );
            }
        }
    }

    #[test]
    fn every_detector_spec_modes_are_known_reachable_modes() {
        // Every mode a detector claims to run in must be one of the modes
        // the CLI can actually dispatch — otherwise a typo (e.g. "workidr")
        // would silently mis-gate a detector while looking like valid
        // registration.
        for spec in DETECTOR_SPECS {
            for mode in spec.modes {
                assert!(
                    ALL_MODES.contains(mode),
                    "detector '{}' lists unknown mode '{}' (not in ALL_MODES)",
                    spec.name,
                    mode
                );
            }
        }
    }

    #[test]
    fn every_detector_spec_has_at_least_one_mode() {
        for spec in DETECTOR_SPECS {
            assert!(
                !spec.modes.is_empty(),
                "detector '{}' has no modes and can never run",
                spec.name
            );
        }
    }

    #[test]
    fn detector_spec_names_are_unique() {
        let mut names: Vec<&str> = DETECTOR_SPECS.iter().map(|spec| spec.name).collect();
        let unique_count = {
            names.sort_unstable();
            names.dedup();
            names.len()
        };
        assert_eq!(
            unique_count,
            DETECTOR_SPECS.len(),
            "duplicate detector name in DETECTOR_SPECS"
        );
    }

    #[test]
    fn read_utf8_head_reads_only_the_requested_prefix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("artifact.txt");
        std::fs::write(&path, "abcdefghijk").unwrap();

        let content = read_utf8_head(&path, 8).unwrap();

        assert_eq!(content, "abcdefgh");
    }

    #[test]
    fn read_utf8_head_preserves_complete_multibyte_prefixes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("artifact.txt");
        std::fs::write(&path, "ééé").unwrap();

        let content = read_utf8_head(&path, 4).unwrap();

        assert_eq!(content, "éé");
    }

    #[test]
    fn read_utf8_head_drops_partial_multibyte_suffix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("artifact.txt");
        std::fs::write(&path, "1234567€").unwrap();

        let content = read_utf8_head(&path, 8).unwrap();

        assert_eq!(content, "1234567");
    }

    #[cfg(unix)]
    #[test]
    fn read_utf8_head_does_not_wait_for_stream_eof_after_limit() {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::process::Command;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let dir = tempdir().unwrap();
        let fifo_path = dir.path().join("artifact.fifo");
        let status = Command::new("mkfifo").arg(&fifo_path).status().unwrap();
        assert!(status.success());

        let (tx, rx) = mpsc::channel();
        let reader_path = fifo_path.clone();
        let reader = thread::spawn(move || {
            tx.send(read_utf8_head(&reader_path, 8)).unwrap();
        });

        let writer = thread::spawn(move || {
            let mut fifo = OpenOptions::new().write(true).open(&fifo_path).unwrap();
            fifo.write_all(b"12345678").unwrap();
            fifo.flush().unwrap();
            thread::sleep(Duration::from_secs(2));
        });

        let content = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("bounded read should finish before the writer closes the fifo")
            .unwrap();
        assert_eq!(content, "12345678");

        writer.join().unwrap();
        reader.join().unwrap();
    }
}
